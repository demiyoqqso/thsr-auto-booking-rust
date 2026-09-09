pub mod cli;
pub mod schema;

use bytes::Bytes;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::fmt::Debug;
use std::fs;
use std::str::FromStr;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn telegram_notify(message: &str) {
    let token = match std::env::var("TELEGRAM_BOT_TOKEN") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            println!("Telegram notification skipped: TELEGRAM_BOT_TOKEN is not set.");
            return;
        }
    };

    let chat_id = match std::env::var("TELEGRAM_CHAT_ID") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            println!("Telegram notification skipped: TELEGRAM_CHAT_ID is not set.");
            return;
        }
    };

    let url = format!("https://api.telegram.org/bot{}/sendMessage", token.trim());
    let body = serde_urlencoded::to_string([
        ("chat_id", chat_id.trim()),
        ("text", message),
    ])
    .unwrap_or_else(|_| format!("chat_id={}&text={}", chat_id.trim(), message));

    let result = Client::new()
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .timeout(Duration::from_secs(15))
        .send();

    match result {
        Ok(resp) if resp.status().is_success() => {
            println!("Telegram notification sent.");
        }
        Ok(resp) => {
            let status = resp.status();
            let detail = resp.text().unwrap_or_default();
            println!("Telegram notification failed: HTTP {} {}", status, detail);
        }
        Err(err) => {
            println!("Telegram notification failed: {}", err);
        }
    }
}

use crate::cli::Args;
use crate::schema::{STATION_MAP, TIME_TABLE, TicketType};

static BASE_URL: &str = "https://irs.thsrc.com.tw";
static BOOKING_PAGE_URL: &str = "https://irs.thsrc.com.tw/IMINT/?locale=tw";
static SUBMIT_FORM_URL: &str = "https://irs.thsrc.com.tw/IMINT/;jsessionid={}?wicket:interface=:0:BookingS1Form::IFormSubmitListener";
static CONFIRM_TRAIN_URL: &str =
    "https://irs.thsrc.com.tw/IMINT/?wicket:interface=:1:BookingS2Form::IFormSubmitListener";
static CONFIRM_TICKET_URL: &str =
    "https://irs.thsrc.com.tw/IMINT/?wicket:interface=:2:BookingS3Form::IFormSubmitListener";

fn get_header() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Host", HeaderValue::from_static("irs.thsrc.com.tw"));
    headers.insert(
        "User-Agent",
        HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:137.0) Gecko/20100101 Firefox/137.0",
        ),
    );
    headers.insert(
        "Accept",
        HeaderValue::from_static(
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
        ),
    );
    headers.insert(
        "Accept-Language",
        HeaderValue::from_static("zh-TW,zh;q=0.8,en-US;q=0.5,en;q=0.3"),
    );
    headers.insert("Accept-Encoding", HeaderValue::from_static("deflate, br"));
    headers.insert("Connection", HeaderValue::from_static("keep-alive"));
    headers.insert("Upgrade-Insecure-Requests", HeaderValue::from_static("1"));
    headers.insert(
        "Referer",
        HeaderValue::from_static("https://irs.thsrc.com.tw/IMINT/"),
    );
    headers.insert("Sec-Fetch-Site", HeaderValue::from_static("same-origin"));
    headers.insert("Sec-Fetch-Mode", HeaderValue::from_static("no-cors"));
    headers
}

fn get_input<T: FromStr>(hint: &str, default: T) -> T {
    println!("{hint}");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap_or_default();
    let input = input.trim().to_string();
    if input.is_empty() {
        return default;
    }
    input.parse().unwrap_or(default)
}

pub fn run(args: Args) {
    // ===== FIXED BOOKING SETTINGS =====
    // These values intentionally override CLI/env booking options so the
    // Railway deployment cannot accidentally revert to an old time setting.
    // Departure window: 07:30 through 12:00 (NOON), inclusive.
    let fixed_args = Args {
        personal_id: args.personal_id.or_else(|| std::env::var("THSR_PERSONAL_ID").ok()),
        date: Some("2026/09/25".to_string()),
        time: Some(6), // TIME_TABLE ID 6 = 07:30
        from: Some(3), // Banqiao
        to: Some(7),   // Taichung
        adult_cnt: Some(2),
        student_cnt: Some(0),
        seat_prefer: Some(0), // any seat
        class_type: Some(0),  // standard class
        use_membership: Some(false),
        retry_seconds: 3,
        list_station: false,
        list_time_table: false,
    };
    let retry_seconds = 3;

    println!("FIXED SETTINGS: Banqiao -> Taichung | date 2026/09/25 | departure 07:30-12:00 NOON | adults 2 | students 0 | seat any | class standard | membership off | retry 3s");

    // Send one startup message so Telegram configuration can be verified
    // immediately instead of waiting for a CAPTCHA or successful booking.
    telegram_notify("🚄 THSR AUTO BOOKING 已啟動\n固定條件：板橋→台中｜2026/09/25｜07:30～12:00（中午12點）｜成人2｜不限座位｜標準車廂｜會員關閉");
    let policy = reqwest::redirect::Policy::limited(20);
    let client = Client::builder()
        .redirect(policy)
        .default_headers(get_header())
        .cookie_store(true)
        .timeout(Duration::from_secs(60))
        .build()
        .expect("Failed to create HTTP client");

    let mut attempt = 0u64;
    loop {
        attempt += 1;
        println!();
        println!("=================================");
        println!("THSR AUTO BOOKING - ATTEMPT {}", attempt);
        println!("=================================");

        match run_once(&client, &fixed_args) {
            Ok(resp) => {
                if !has_booking_result(&resp) {
                    println!("Booking flow finished without a PNR. The site may have rejected the request.");
                    println!("Retrying in {} seconds...", retry_seconds);
                    std::thread::sleep(Duration::from_secs(retry_seconds));
                    continue;
                }

                println!("=================================");
                println!("BOOKING SUCCESS!");
                println!("=================================");
                show_result(&resp);
                break;
            }
            Err(err) => {
                println!("Booking attempt failed: {}", err);
                println!("Retrying in {} seconds...", retry_seconds);
                std::thread::sleep(Duration::from_secs(retry_seconds));
            }
        }
    }
}

fn run_once(client: &Client, args: &Args) -> Result<Html, String> {
    let resp = booking_flow::run_flow(client, args)?;
    let resp = confirm_train_flow::run_flow(resp, client)?;
    confirm_ticket_flow::run_flow(&resp, client, args)
}


fn has_booking_result(page: &Html) -> bool {
    Selector::parse("p.pnr-code span")
        .ok()
        .and_then(|selector| page.select(&selector).next())
        .is_some()
}

pub fn parse_error(page: &Html) -> Option<String> {
    let err_selector = Selector::parse("span.feedbackPanelERROR").unwrap();
    let errors: Vec<String> = page
        .select(&err_selector)
        .filter_map(|element| element.text().next().map(|text| text.trim().to_string()))
        .collect();
    if errors.is_empty() {
        None
    } else {
        Some(errors.join("\n"))
    }
}

// First page: Booking Flow
pub mod booking_flow {
    use super::*;

    pub fn run_flow(client: &Client, args: &Args) -> Result<Html, String> {
        println!("Requesting booking page...");
        let response = client.get(BOOKING_PAGE_URL).send().unwrap();

        // Parse jsession id
        let jid = response
            .cookies()
            .find(|cookie| cookie.name() == "JSESSIONID")
            .map(|cookie| cookie.value().to_string())
            .unwrap();

        // Parse to HTML object
        let body = response.text().unwrap(); // Get the response body as a string
        let document = Html::parse_document(&body);

        // Request security code image
        let sec_code_img_url = parse_security_code_img_url(&document);
        let img_resp = client.get(&sec_code_img_url).send().unwrap();

        // Making selections
        let mut payload = BookingPayload::default();
        payload.search_by = parse_search_by(&document);
        payload.types_of_trip = parse_types_of_trip_value(&document);
        payload.select_start_station(&args.from);
        payload.select_dest_station(&args.to);
        let (start_date, end_date) = parse_avail_start_end_date(&document);
        payload.select_date(&start_date, &end_date, &args.date);
        payload.select_time(&args.time);
        if args.adult_cnt.is_none() && args.student_cnt.is_none() {
            payload.select_ticket_num(TicketType::Adult, &None);
        }
        if args.adult_cnt.is_some() {
            payload.select_ticket_num(TicketType::Adult, &args.adult_cnt);
        }
        if args.student_cnt.is_some() {
            payload.select_ticket_num(TicketType::College, &args.student_cnt);
        }
        payload.select_seat_prefer(&args.seat_prefer);
        payload.select_class_type(&args.class_type);
        payload.input_security_code(img_resp.bytes().unwrap());

        // Make the booking request
        let resp = client
            .post(SUBMIT_FORM_URL.replace("{}", &jid))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(serde_urlencoded::to_string(&payload).unwrap())
            .send()
            .unwrap();

        // Parse to HTML object
        let resp_html = Html::parse_document(&resp.text().unwrap());
        if let Some(err_msg) = parse_error(&resp_html) {
            return Err(err_msg);
        }
        Ok(resp_html)
    }

    fn parse_avail_start_end_date(page: &Html) -> (String, String) {
        let selector = Selector::parse("#toTimeInputField").unwrap();
        let elem = page.select(&selector).next().unwrap();
        let end_date = elem.attr("limit").unwrap();
        let start_date = elem.attr("date").unwrap();
        (start_date.to_string(), end_date.to_string())
    }

    fn parse_types_of_trip_value(page: &Html) -> u8 {
        let selector = Selector::parse("#BookingS1Form_tripCon_typesoftrip").unwrap();
        let elem = page.select(&selector).next().unwrap();
        let selected_selector = Selector::parse("[selected='selected']").unwrap();
        let trip_type = elem.select(&selected_selector).next().unwrap();
        trip_type.attr("value").unwrap().parse().unwrap()
    }

    fn parse_search_by(page: &Html) -> String {
        let candidates_selector = Selector::parse("input[name='bookingMethod']").unwrap();
        let candidates = page.select(&candidates_selector);
        let tag = candidates
            .filter(|cand| cand.value().attr("checked").is_some())
            .next()
            .unwrap();
        tag.value().attr("value").unwrap().to_string()
    }

    fn parse_security_code_img_url(page: &Html) -> String {
        let selector = Selector::parse("#BookingS1Form_homeCaptcha_passCode").unwrap();
        let elem = page.select(&selector).next().unwrap();
        let img_url = elem.attr("src").unwrap();
        format!("{}{}", BASE_URL, img_url)
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub struct BookingPayload {
        #[serde(rename(serialize = "selectStartStation"))]
        pub start_station: u8,

        #[serde(rename(serialize = "selectDestinationStation"))]
        pub dest_station: u8,

        #[serde(rename(serialize = "bookingMethod"))]
        pub search_by: String,

        #[serde(rename(serialize = "tripCon:typesoftrip"), default)]
        pub types_of_trip: u8, // 0: one way, 1: round trip

        #[serde(rename(serialize = "toTimeInputField"))]
        pub outbound_date: String,

        #[serde(rename(serialize = "toTimeTable"))]
        pub outbound_time: String,

        #[serde(rename(serialize = "homeCaptcha:securityCode"))]
        pub security_code: String,

        #[serde(rename(serialize = "seatCon:seatRadioGroup"))]
        pub seat_prefer: usize, // 0: any, 1: window, 2: aisle

        #[serde(rename(serialize = "BookingS1Form:hf:0"), default)]
        pub form_mark: String,

        #[serde(rename(serialize = "trainCon:trainRadioGroup"), default)]
        pub class_type: u8, // 0: standard, 1: business

        #[serde(rename(serialize = "backTimeInputField"))]
        pub inbound_date: Option<String>,

        #[serde(rename(serialize = "backTimeTable"))]
        pub inbound_time: Option<String>,

        #[serde(rename(serialize = "toTrainIDInputField"), default)]
        pub to_train_id: Option<u8>,

        #[serde(rename(serialize = "backTrainIDInputField"), default)]
        pub back_train_id: Option<u8>,

        #[serde(
            rename(serialize = "ticketPanel:rows:0:ticketAmount"),
            default = "default_adult_ticket_num"
        )]
        pub adult_ticket_num: String,

        #[serde(
            rename(serialize = "ticketPanel:rows:1:ticketAmount"),
            default = "default_child_ticket_num"
        )]
        pub child_ticket_num: String,

        #[serde(
            rename(serialize = "ticketPanel:rows:2:ticketAmount"),
            default = "default_disabled_ticket_num"
        )]
        pub disabled_ticket_num: String,

        #[serde(
            rename(serialize = "ticketPanel:rows:3:ticketAmount"),
            default = "default_elder_ticket_num"
        )]
        pub elder_ticket_num: String,

        #[serde(
            rename(serialize = "ticketPanel:rows:4:ticketAmount"),
            default = "default_college_ticket_num"
        )]
        pub college_ticket_num: String,
    }

    pub fn default_adult_ticket_num() -> String {
        "1F".to_string()
    }

    pub fn default_child_ticket_num() -> String {
        "0H".to_string()
    }

    pub fn default_disabled_ticket_num() -> String {
        "0W".to_string()
    }

    pub fn default_elder_ticket_num() -> String {
        "0E".to_string()
    }

    pub fn default_college_ticket_num() -> String {
        "0P".to_string()
    }

    impl Default for BookingPayload {
        fn default() -> Self {
            BookingPayload {
                start_station: 1,
                dest_station: 12,
                search_by: "1".to_string(),
                types_of_trip: 0,
                outbound_date: "2023/10/01".to_string(),
                outbound_time: "08:00".to_string(),
                security_code: "1234".to_string(),
                seat_prefer: 0,
                form_mark: "".to_string(),
                class_type: 0,
                inbound_date: None,
                inbound_time: None,
                to_train_id: None,
                back_train_id: None,
                adult_ticket_num: default_adult_ticket_num(),
                child_ticket_num: default_child_ticket_num(),
                disabled_ticket_num: default_disabled_ticket_num(),
                elder_ticket_num: default_elder_ticket_num(),
                college_ticket_num: default_college_ticket_num(),
            }
        }
    }

    impl BookingPayload {
        pub fn select_start_station(&mut self, from: &Option<usize>) {
            if let Some(from) = from {
                self.start_station = from.clone() as u8;
                return;
            }

            for (i, station) in STATION_MAP.iter().enumerate() {
                println!("{}: {:?}", i + 1, station);
            }
            let input = get_input("Please select start station (default: 1):", 1);
            if input > 0 && input <= STATION_MAP.len() {
                self.start_station = input as u8;
            } else {
                println!("Invalid input, defaulting to Nangang.");
                self.start_station = 1;
            }
        }

        pub fn select_dest_station(&mut self, to: &Option<usize>) {
            if let Some(to) = to {
                self.dest_station = to.clone() as u8;
                return;
            }

            for (i, station) in STATION_MAP.iter().enumerate() {
                println!("{}: {:?}", i + 1, station);
            }
            let input = get_input("Please select destination station (default: 12):", 12);
            if input > 0 && input <= STATION_MAP.len() {
                self.dest_station = input as u8;
            } else {
                println!("Invalid input, defaulting to Zuouing.");
                self.dest_station = 12;
            }
        }

        pub fn input_security_code(&mut self, img_data: Bytes) {
            self.security_code = wait_for_captcha(&img_data);
        }

        pub fn select_date(
            &mut self,
            start_date: &String,
            end_date: &String,
            date: &Option<String>,
        ) {
            let input = match date.clone() {
                Some(date) => date,
                None => get_input(
                    &format!(
                        "Please select a date between {} and {} (default to {}):",
                        start_date, end_date, start_date
                    ),
                    start_date.clone(),
                ),
            };

            let input = match normalize_date(&input) {
                Some(date) => date,
                None => {
                    println!("Invalid date format, defaulting to {}", start_date);
                    start_date.clone()
                }
            };

            if input.is_empty() {
                self.outbound_date = start_date.clone();
                return;
            }

            if input.ge(start_date) && input.le(end_date) {
                self.outbound_date = input;
            } else {
                println!("Invalid date, defaulting to {}", start_date);
                self.outbound_date = start_date.to_string();
            }
        }

        pub fn select_time(&mut self, time: &Option<usize>) {
            let opt = match time.clone() {
                Some(time) => time,
                None => {
                    for (idx, &t_str) in TIME_TABLE.iter().enumerate() {
                        let mut t_int = t_str[..t_str.len() - 1].parse::<u16>().unwrap();
                        if t_str.ends_with('A') && (t_int / 100) == 12 {
                            t_int %= 1200;
                        } else if t_int != 1230 && t_str.ends_with('P') {
                            t_int += 1200;
                        }
                        let formatted_time = format!("{:04}", t_int);
                        println!(
                            "{}. {}:{}",
                            idx + 1,
                            &formatted_time[..formatted_time.len() - 2],
                            &formatted_time[formatted_time.len() - 2..]
                        );
                    }
                    get_input("Select departure time (default: 10):", 10)
                }
            };

            if opt == 0 || opt > TIME_TABLE.len() {
                println!("Invalid input, defaulting to 10.");
                self.outbound_time = TIME_TABLE[9].to_string();
                return;
            }

            self.outbound_time = TIME_TABLE[opt - 1].to_string();
        }

        pub fn select_ticket_num(&mut self, ticket_type: TicketType, val: &Option<u8>) {
            let mut val = match val.clone() {
                Some(val) => val,
                None => get_input(
                    &format!(
                        "Please select the number (0~10) of tickets for {:?} (default: 1)",
                        ticket_type
                    ),
                    1,
                ),
            };

            if val > 10 {
                println!("Invalid input, defaulting to 1.");
                val = 1;
            }

            let val = format!("{}{}", val, (ticket_type.clone() as u8) as char);
            match ticket_type {
                TicketType::Adult => self.adult_ticket_num = val,
                TicketType::Child => self.child_ticket_num = val,
                TicketType::Disabled => self.disabled_ticket_num = val,
                TicketType::Elder => self.elder_ticket_num = val,
                TicketType::College => self.college_ticket_num = val,
            }
        }

        pub fn select_seat_prefer(&mut self, prefer: &Option<usize>) {
            let input = match prefer.clone() {
                Some(prefer) => prefer,
                None => get_input(
                    "Please select seat preference (0: any, 1: window, 2: aisle) (default: 0):",
                    0,
                ),
            };

            if input > 2 {
                println!("Invalid input, defaulting to any.");
                self.seat_prefer = 0;
            } else {
                self.seat_prefer = input;
            }
        }

        pub fn select_class_type(&mut self, class_type: &Option<usize>) {
            let input = match class_type.clone() {
                Some(class_type) => class_type,
                None => get_input(
                    "Please select class type (0: standard, 1: business) (default: 0):",
                    0,
                ),
            };

            if input > 1 {
                println!("Invalid input, defaulting to standard.");
                self.class_type = 0;
            } else {
                self.class_type = input as u8;
            }
        }
    }

    fn normalize_date(input: &str) -> Option<String> {
        let parts: Vec<&str> = input.split('/').collect();
        if parts.len() != 3 {
            return None;
        }

        let year = parts[0].parse::<u16>().ok()?;
        let month = parts[1].parse::<u8>().ok()?;
        let day = parts[2].parse::<u8>().ok()?;

        if year >= 1000 && month >= 1 && month <= 12 && day >= 1 && day <= 31 {
            Some(format!("{:04}/{:02}/{:02}", year, month, day))
        } else {
            None
        }
    }

fn wait_for_captcha(img_data: &[u8]) -> String {
    fs::write("tmp_code.jpg", img_data)
        .expect("Failed to write captcha image");

    let token = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );

    let state = Arc::new((Mutex::new(None::<String>), Condvar::new()));

    // Railway provides the listening port through PORT. Locally we use an
    // ephemeral port so multiple copies can run without conflicts.
    let port = std::env::var("PORT").unwrap_or_else(|_| "0".to_string());
    let listener = TcpListener::bind(format!("0.0.0.0:{port}"))
        .or_else(|_| TcpListener::bind("127.0.0.1:0"))
        .expect("Failed to start CAPTCHA web server");
    let addr = listener
        .local_addr()
        .expect("Failed to read CAPTCHA server address");

    let state_for_thread = Arc::clone(&state);
    let image = img_data.to_vec();
    let token_for_thread = token.clone();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = handle_captcha_request(
                &mut stream,
                &token_for_thread,
                &image,
                &state_for_thread,
            );

            if state_for_thread
                .0
                .lock()
                .map(|g| g.is_some())
                .unwrap_or(true)
            {
                break;
            }
        }
    });

    // On Railway the browser is NOT inside this container, so we must give
    // the user a public URL. Prefer an explicit URL, then Railway's generated
    // public domain.
    // Public URL for the SAME Railway service. No separate captcha service is
    // required. THSR_PUBLIC_URL is preferred; Railway's generated public
    // domain is used automatically when available.
    let public_url = std::env::var("THSR_PUBLIC_URL")
        .or_else(|_| std::env::var("RAILWAY_PUBLIC_DOMAIN"))
        .or_else(|_| std::env::var("RAILWAY_STATIC_URL"))
        .ok()
        .map(|value| {
            let value = value.trim().trim_end_matches('/');
            if value.starts_with("http://") || value.starts_with("https://") {
                value.to_string()
            } else {
                format!("https://{value}")
            }
        });

    let is_railway = std::env::var_os("RAILWAY_ENVIRONMENT_NAME").is_some()
        || std::env::var_os("RAILWAY_PROJECT_ID").is_some();

    let captcha_url = match public_url {
        Some(ref base) => format!("{}/captcha/{}/", base.trim_end_matches('/'), token),
        None if is_railway => {
            println!();
            println!("=================================");
            println!("CAPTCHA URL NOT AVAILABLE");
            println!("=================================");
            println!("The CAPTCHA server is running on Railway, but this service has no public URL.");
            println!("1. Railway -> Service -> Settings -> Networking -> Generate Domain");
            println!("2. Redeploy the service");
            println!("3. Or set THSR_PUBLIC_URL=https://YOUR-DOMAIN in Railway Variables");
            println!("=================================");
            return String::new();
        }
        None => format!("http://127.0.0.1:{}/captcha/{}/", addr.port(), token),
    };

    println!();
    println!("=================================");
    println!("CAPTCHA REQUIRED");
    println!("=================================");
    println!("Open this URL in your browser:");
    println!("{captcha_url}");
    println!("Enter the CAPTCHA and press Submit.");
    println!("=================================");

    // Send the CAPTCHA URL to Telegram immediately so the user can open it
    // from a phone without watching Railway logs.
    println!("TELEGRAM: sending CAPTCHA URL...");
    telegram_notify(&format!(
        "🚄 THSR CAPTCHA REQUIRED\n\n請開啟以下網址輸入驗證碼：\n{}\n\n完成後按送出，程式會繼續搶票。",
        captcha_url
    ));

    // When running directly on a desktop, also try to open the URL for the
    // user. This is deliberately skipped on Railway/headless environments.
    if !is_railway && public_url.is_none() {
        open_in_browser(&captcha_url);
    }

    let (lock, cvar) = &*state;
    let mut code = lock.lock().expect("captcha state poisoned");
    while code.is_none() {
        code = cvar.wait(code).expect("captcha state poisoned");
    }
    code.take().unwrap_or_default()
}

fn open_in_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

fn handle_captcha_request(
    stream: &mut TcpStream,
    token: &str,
    image: &[u8],
    state: &Arc<(Mutex<Option<String>>, Condvar)>,
) -> std::io::Result<()> {
    // Railway's HTTP proxy can deliver request headers and POST data in
    // separate TCP packets, so read the complete headers first.
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;

    let mut buffer = Vec::<u8>::with_capacity(8192);
    let mut chunk = [0u8; 4096];

    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            return Ok(());
        }
    }

    let header_end = match buffer.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(pos) => pos + 4,
        None => return Ok(()),
    };

    let (first_line, content_length) = {
        let headers = String::from_utf8_lossy(&buffer[..header_end]);
        let first_line = headers.lines().next().unwrap_or_default().to_string();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        (first_line, content_length)
    };

    while buffer.len() < header_end.saturating_add(content_length) {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.len() > 2 * 1024 * 1024 {
            return Ok(());
        }
    }

    let request_path = first_line.split_whitespace().nth(1).unwrap_or("/");
    let expected_path = format!("/captcha/{token}/");

    if request_path != expected_path {
        return write_http(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"Not Found",
        );
    }

    if first_line.starts_with("GET ") {
        let html = captcha_html(token, image);
        return write_http(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            html.as_bytes(),
        );
    }

    if first_line.starts_with("POST ") {
        let body_end = header_end.saturating_add(content_length).min(buffer.len());
        let body = String::from_utf8_lossy(&buffer[header_end..body_end]);
        let code = form_value(&body, "code").trim().to_string();

        if code.is_empty() {
            let html = captcha_html(token, image);
            return write_http(
                stream,
                "400 Bad Request",
                "text/html; charset=utf-8",
                html.as_bytes(),
            );
        }

        {
            let (lock, cvar) = &**state;
            let mut guard = lock.lock().expect("captcha state poisoned");
            *guard = Some(code);
            cvar.notify_one();
        }

        let html = "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>THSR CAPTCHA</title></head><body style=\"font-family:sans-serif;max-width:520px;margin:40px auto;padding:20px\"><h2>驗證碼已送出</h2><p>請回到 Railway 查看搶票結果；程式會繼續執行。</p></body></html>";
        return write_http(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            html.as_bytes(),
        );
    }

    write_http(
        stream,
        "405 Method Not Allowed",
        "text/plain; charset=utf-8",
        b"Method Not Allowed",
    )
}

fn captcha_html(token: &str, image: &[u8]) -> String {
    // Embed the CAPTCHA directly in the HTML as a data URI. This avoids a
    // second browser request for /image, which is especially important when
    // the program is running behind Railway's public proxy.
    let image_b64 = base64_encode(image);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>THSR CAPTCHA</title></head><body style=\"font-family:sans-serif;max-width:520px;margin:40px auto;padding:20px\"><h2>台灣高鐵驗證碼</h2><p>請看圖片輸入驗證碼：</p><img src=\"data:image/jpeg;base64,{image_b64}\" alt=\"CAPTCHA\" style=\"max-width:100%;image-rendering:auto;border:1px solid #ccc\"><form method=\"post\" action=\"/captcha/{token}/\" style=\"margin-top:20px\"><input name=\"code\" autocomplete=\"off\" autofocus style=\"font-size:24px;width:180px\"><button type=\"submit\" style=\"font-size:20px;margin-left:8px\">送出</button></form></body></html>"
    )
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().saturating_add(2) / 3 * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };

        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[((b0 & 0b0000_0011) << 4 | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((b1 & 0b0000_1111) << 2 | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }

    out
}

fn form_value(body: &str, key: &str) -> String {
    body.split('&')
        .find_map(|pair| {
            let mut it = pair.splitn(2, '=');
            let k = it.next()?;
            let v = it.next().unwrap_or_default();
            if k == key { Some(url_decode(v)) } else { None }
        })
        .unwrap_or_default()
}

fn url_decode(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn write_http(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)
}
}

// Second page: Confirm Train Flow
pub mod confirm_train_flow {
    use super::*;

    pub fn run_flow(document: Html, client: &Client) -> Result<Html, String> {
        let alerts = parse_alert_body(&document);
        if !alerts.is_empty() {
            println!("{}", alerts.join("\n"));
        }

        let trains = parse_trains(&document);
        let mut payload = ConfirmTrainPayload::default();
        payload.select_available_trains(trains.as_slice())?;

        let resp = client
            .post(CONFIRM_TRAIN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(serde_urlencoded::to_string(&payload).unwrap())
            .send()
            .unwrap();

        // Parse to HTML object
        let resp_html = Html::parse_document(&resp.text().unwrap());
        if let Some(err_msg) = parse_error(&resp_html) {
            return Err(err_msg);
        }
        Ok(resp_html)
    }

    fn parse_alert_body(document: &Html) -> Vec<String> {
        let li_selector = Selector::parse("ul.alert-body > li").unwrap();
        document
            .select(&li_selector)
            .map(|tag| tag.text().collect::<Vec<_>>().join("").trim().to_string())
            .collect()
    }

    fn parse_trains(document: &Html) -> Vec<Train> {
        let selector = Selector::parse("label.result-item").unwrap(); // Adjust the selector based on `self.cond.from_html`
        let avail = document.select(&selector);

        avail
            .map(|element| {
                let tag_selector = Selector::parse("input").unwrap();
                let elem = element.select(&tag_selector).next().unwrap();

                let id = elem.attr("querycode").unwrap().parse().unwrap();
                let depart = elem.attr("querydeparture").unwrap().to_string();
                let arrive = elem.attr("queryarrival").unwrap().to_string();
                let travel_time = elem.attr("queryestimatedtime").unwrap().to_string();
                let form_value = elem.attr("value").unwrap().to_string();
                let discount_info = parse_discount(&element);

                Train {
                    id,
                    depart,
                    arrive,
                    travel_time,
                    discount_info,
                    form_value,
                }
            })
            .collect()
    }

    fn parse_discount(item: &scraper::ElementRef) -> String {
        let mut discounts = Vec::new();

        if let Some(tag) = item
            .select(&Selector::parse("p.early-bird span").unwrap())
            .next()
        {
            discounts.push(tag.text().next().unwrap().to_string());
        }

        if let Some(tag) = item
            .select(&Selector::parse("p.student span").unwrap())
            .next()
        {
            discounts.push(tag.text().next().unwrap().to_string());
        }

        if !discounts.is_empty() {
            format!("({})", discounts.join(", "))
        } else {
            String::new()
        }
    }

    #[derive(Debug)]
    pub struct Train {
        id: u32,
        depart: String,
        arrive: String,
        travel_time: String,
        discount_info: String,
        form_value: String,
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub struct ConfirmTrainPayload {
        #[serde(rename(serialize = "TrainQueryDataViewPanel:TrainGroup"), default)]
        pub selected_train: String,

        #[serde(rename(serialize = "BookingS2Form:hf:0"), default)]
        pub form_mark: String,
    }

    impl Default for ConfirmTrainPayload {
        fn default() -> Self {
            ConfirmTrainPayload {
                selected_train: "".to_string(),
                form_mark: "".to_string(),
            }
        }
    }

    fn departure_minutes(value: &str) -> Option<u16> {
        let (h, m) = value.trim().split_once(':')?;
        let hour = h.parse::<u16>().ok()?;
        let minute = m.parse::<u16>().ok()?;
        if hour <= 23 && minute <= 59 {
            Some(hour * 60 + minute)
        } else {
            None
        }
    }

    impl ConfirmTrainPayload {
pub fn select_available_trains(&mut self, trains: &[Train]) -> Result<(), String> {
    if trains.is_empty() {
        println!("No available trains.");
        return Err("NO_TRAIN_AVAILABLE".to_string());
    }

    println!();
    println!("===== AVAILABLE TRAIN =====");
    for (idx, train) in trains.iter().enumerate() {
        println!(
            "{:>2}. {:>4} {:>5}~{:>5} {:>4} {}",
            idx + 1,
            train.id,
            train.depart,
            train.arrive,
            train.travel_time,
            train.discount_info
        );
    }

    // HARD STOP: only accept departures from 07:30 through 12:00 (NOON).
    // Never select a train after noon, even if the THSR response contains one.
    // The lower bound also prevents an accidental selection of overnight/early
    // morning trains if the server returns a wider result set.
    const MIN_DEPARTURE_MINUTES: u16 = 7 * 60 + 30;
    const MAX_DEPARTURE_MINUTES: u16 = 12 * 60;

    let selected = trains.iter().find(|train| {
        departure_minutes(&train.depart)
            .map(|m| m >= MIN_DEPARTURE_MINUTES && m <= MAX_DEPARTURE_MINUTES)
            .unwrap_or(false)
    });

    let selected = match selected {
        Some(train) => train,
        None => {
            println!("No available trains in the fixed 07:30-12:00 NOON window.");
            return Err("NO_TRAIN_IN_FIXED_TIME_WINDOW".to_string());
        }
    };
    println!(
        "AUTO SELECT: Train {} {} -> {}",
        selected.id, selected.depart, selected.arrive
    );
    self.selected_train = selected.form_value.clone();
    Ok(())
}
    }
}

// Final page: Confirm Ticket Flow
pub mod confirm_ticket_flow {
    use super::*;

    pub fn run_flow(document: &Html, client: &Client, args: &Args) -> Result<Html, String> {
        // let body = fs::read_to_string("confirm_response.html").unwrap();
        // let body = std::fs::read_to_string("confirm_ticket_super_early_bird.html").unwrap();

        let mut payload = ConfirmTicketPayload::default();

        // Input personal ID
        let personal_id = payload.input_personal_id(&args.personal_id);

        // Parse membership radio
        let (radio_value, add_payload) =
            process_membership(&document, &personal_id, &args.use_membership);
        payload.member_radio = radio_value;

        // Additional flow for early bird
        let mut payload = serde_urlencoded::to_string(&payload).unwrap();
        if let Some(additional_payload) = process_early_bird(&document, &personal_id) {
            let additional_payload = serde_urlencoded::to_string(&additional_payload).unwrap();
            payload = format!("{}&{}", payload, additional_payload);
        }

        if let Some(add_payload) = add_payload {
            payload = format!("{}&{}", payload, add_payload);
        }

        println!("Booking...");
        let resp = client
            .post(CONFIRM_TICKET_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(payload)
            .send()
            .unwrap();

        let html = Html::parse_document(&resp.text().unwrap());
        if let Some(err_msg) = parse_error(&html) {
            return Err(err_msg);
        }
        Ok(html)
    }

    #[derive(Serialize, Deserialize, Debug)]
    struct ConfirmTicketPayload {
        #[serde(rename(serialize = "dummyId"))]
        pub personal_id: String,

        #[serde(rename(serialize = "dummyPhone"))]
        pub phone_num: String,

        #[serde(rename(
            serialize = "TicketMemberSystemInputPanel:TakerMemberSystemDataView:memberSystemRadioGroup"
        ))]
        pub member_radio: String, // 非高鐵會員, 企業會員 / 高鐵會員 / 企業會員統編

        #[serde(rename(serialize = "BookingS3FormSP:hf:0"), default)]
        form_mark: String,

        #[serde(rename(serialize = "idInputRadio"), default)]
        id_input_radio: u8, // 0: 身份證字號 / 1: 護照號碼

        #[serde(rename(serialize = "diffOver"), default = "default_diff_over")]
        diff_over: u8,

        #[serde(rename(serialize = "email"), default)]
        email: String,

        #[serde(rename(serialize = "agree"), default = "default_agree")]
        agree: String,

        #[serde(rename(serialize = "isGoBackM"), default)]
        go_back_m: String,

        #[serde(rename(serialize = "backHome"), default)]
        back_home: String,

        #[serde(rename(serialize = "TgoError"), default)]
        tgo_error: u8,
    }

    fn default_diff_over() -> u8 {
        1
    }

    fn default_agree() -> String {
        "on".to_string()
    }

    impl Default for ConfirmTicketPayload {
        fn default() -> Self {
            ConfirmTicketPayload {
                personal_id: "".to_string(),
                phone_num: "".to_string(),
                member_radio: "0".to_string(),
                form_mark: "".to_string(),
                id_input_radio: 0,
                diff_over: default_diff_over(),
                email: "".to_string(),
                agree: default_agree(),
                go_back_m: "".to_string(),
                back_home: "".to_string(),
                tgo_error: 1,
            }
        }
    }

    impl ConfirmTicketPayload {
        pub fn input_personal_id(&mut self, personal_id: &Option<String>) -> String {
            let input = match personal_id.clone() {
                Some(id) => id,
                None => {
                    println!("Input personal ID:");
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input).unwrap_or_default();
                    let input: String = input.trim().to_string();
                    input
                }
            };

            self.personal_id = input.trim().to_string();
            self.personal_id.clone()
        }
    }

    fn process_membership(
        page: &Html,
        membership_id: &String,
        to_use_membership: &Option<bool>,
    ) -> (String, Option<String>) {
        let use_membership = match to_use_membership {
            Some(v) => *v,
            None => {
                match get_input("Use membership (y/n, default: n):", "n".to_string()).as_str() {
                    "y" => true,
                    _ => false,
                }
            }
        };

        let sel_str = match use_membership {
            true => "#memberSystemRadio1",
            false => "#memberSystemRadio3",
        };

        let membership_selector = Selector::parse(sel_str).unwrap();
        let elem = page.select(&membership_selector).next().unwrap();
        let membership_radio = elem.attr("value").unwrap();

        if use_membership {
            let payload = vec![
                (
                    "TicketMemberSystemInputPanel:TakerMemberSystemDataView:memberSystemRadioGroup:memberShipNumber",
                    membership_id.clone(),
                ),
                (
                    "TicketMemberSystemInputPanel:TakerMemberSystemDataView:memberSystemRadioGroup:memberSystemShipCheckBox",
                    "on".to_string(),
                ),
            ];
            let encoded_payload = serde_urlencoded::to_string(&payload).unwrap();
            return (membership_radio.to_string(), Some(encoded_payload));
        }

        (membership_radio.to_string(), None)
    }

    fn process_early_bird(page: &Html, personal_id: &str) -> Option<HashMap<String, String>> {
        let selector = Selector::parse(".superEarlyBird").unwrap();
        let elem: Vec<String> = page
            .select(&selector)
            .filter_map(|tag| tag.text().next().map(|text| text.to_string()))
            .collect();

        if elem.is_empty() {
            return None;
        }

        let personal_id = get_input(
            &format!("Passenger's ID number (default: {}):", personal_id),
            personal_id.to_string(),
        );

        let early_type_selector = Selector::parse(
            "input[name='TicketPassengerInfoInputPanel:passengerDataView:0:passengerDataView2:passengerDataTypeName']").unwrap();
        let early_type_elem = page.select(&early_type_selector).next().unwrap();
        let early_type = early_type_elem.attr("value").unwrap().to_string();

        let mut additional_payload = HashMap::from([
            (
                "TicketPassengerInfoInputPanel:passengerDataView:0:passengerDataView2:passengerDataLastName".to_string(),
                "".to_string(),
            ),
            (
                "TicketPassengerInfoInputPanel:passengerDataView:0:passengerDataView2:passengerDataFirstName".to_string(),
                "".to_string(),
            ),
            (
                "TicketPassengerInfoInputPanel:passengerDataView:0:passengerDataView2:passengerDataTypeName".to_string(),
                early_type.clone(),
            ),
            (
                "TicketPassengerInfoInputPanel:passengerDataView:0:passengerDataView2:passengerDataIdNumber".to_string(),
                personal_id,
            ),
            (
                "TicketPassengerInfoInputPanel:passengerDataView:0:passengerDataView2:passengerDataInputChoice".to_string(),
                "0".to_string(), // 0 for ID, 1 for passport
            ),
        ]);

        for i in 1..elem.len() {
            let inp_id = loop {
                let inp_id = get_input(
                    &format!(
                        "Input passenger's ID for passenger {}\n(ID change is not allowed after input!):",
                        i + 1
                    ),
                    "".to_string(),
                );
                if inp_id.is_empty() {
                    println!("ID should not be empty!");
                } else {
                    break inp_id;
                }
            };

            additional_payload.insert(
                format!("TicketPassengerInfoInputPanel:passengerDataView:{i}:passengerDataView2:passengerDataLastName"),
                "".to_string(),
            );
            additional_payload.insert(
                format!("TicketPassengerInfoInputPanel:passengerDataView:{i}:passengerDataView2:passengerDataFirstName"),
                "".to_string(),
            );
            additional_payload.insert(
                format!("TicketPassengerInfoInputPanel:passengerDataView:{i}:passengerDataView2:passengerDataTypeName"),
                early_type.clone(),
            );
            additional_payload.insert(
                format!("TicketPassengerInfoInputPanel:passengerDataView:{i}:passengerDataView2:passengerDataIdNumber"),
                inp_id.trim().to_string(),
            );
            additional_payload.insert(
                format!("TicketPassengerInfoInputPanel:passengerDataView:{i}:passengerDataView2:passengerDataInputChoice"),
                "0".to_string(), // 0 for ID, 1 for passport
            );
        }
        Some(additional_payload)
    }
}

fn show_result(page: &Html) {
    let pnr_code_selector = Selector::parse("p.pnr-code span").unwrap();
    let pnr_code_span_tag = page.select(&pnr_code_selector).next().unwrap();
    let pnr_code = pnr_code_span_tag.text().next().unwrap();

    println!("\nPlease use the following PNR code for payment and picking up the ticket:");
    println!("PNR Code: {}", pnr_code);

    telegram_notify(&format!(
        "🎉 THSR 訂票成功！\n\nPNR Code: {}\n\n請盡快完成付款。",
        pnr_code.trim()
    ));

    // Price
    let price_selector = Selector::parse("#setTrainTotalPriceValue").unwrap();
    let price_tag = page.select(&price_selector).next().unwrap();
    let price = price_tag.text().next().unwrap();

    let payment_status_selector = Selector::parse("span.status-unpaid span:nth-child(3)").unwrap();
    let payment_status_tag = page.select(&payment_status_selector).next().unwrap();
    let payment_exp_date = payment_status_tag.text().next().unwrap();
    println!("Price: {}. Please pay before {}", price, payment_exp_date);
    println!("-------(Ticket Information)-------");

    // Departure date
    let depart_date_selector = Selector::parse("span.date span").unwrap();
    let depart_date_tag = page.select(&depart_date_selector).next().unwrap();
    let depart_date = depart_date_tag.text().next().unwrap();
    println!("{:>7}{}", "Date: ", depart_date);

    // Departure and arrival time
    let depart_time_selector = Selector::parse("#setTrainDeparture0").unwrap();
    let depart_time_tag = page.select(&depart_time_selector).next().unwrap();
    let depart_time = depart_time_tag.text().next().unwrap();

    let arrive_time_selector = Selector::parse("#setTrainArrival0").unwrap();
    let arrive_time_tag = page.select(&arrive_time_selector).next().unwrap();
    let arrive_time = arrive_time_tag.text().next().unwrap();

    println!(
        "{:>7}{}",
        "Time: ",
        format!("{}~{}", depart_time, arrive_time)
    );

    // Station
    let depart_from_selector = Selector::parse("p.departure-stn span").unwrap();
    let depart_from_tag = page.select(&depart_from_selector).next().unwrap();
    let depart_from = depart_from_tag.text().next().unwrap();
    println!("{:>7}{}", "From: ", depart_from);

    let arrive_to_selector = Selector::parse("p.arrival-stn span").unwrap();
    let arrive_to_tag = page.select(&arrive_to_selector).next().unwrap();
    let arrive_to = arrive_to_tag.text().next().unwrap();
    println!("{:>7}{}", "To: ", arrive_to);

    // Seat info
    let seats_selector = Selector::parse("div.seat-label span").unwrap();
    let seats: Vec<String> = page
        .select(&seats_selector)
        .filter_map(|tag| tag.text().next().map(|text| text.to_string()))
        .collect();

    let passenger_count_selector = Selector::parse("div.uk-accordion-content span").unwrap();
    let passenger_count_tag = page.select(&passenger_count_selector).next().unwrap();
    let passenger_count = passenger_count_tag.text().next().unwrap();

    let seat_type_selector = Selector::parse("p.info-data span").unwrap();
    let seat_type_tag = page.select(&seat_type_selector).next().unwrap();
    let seat_type = seat_type_tag.text().next().unwrap();
    println!("Class: {}{}", seat_type, passenger_count);
    println!("Seats: {}", seats.join(", "));
}
