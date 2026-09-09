# THSR Auto Booking (Rust)

Based on `BreezeWhite/thsr-ticket-rs`, modified for a headless Railway/container environment.

## What changed

- No Python required.
- Automatically selects the first available train.
- Automatically retries when no train is available.
- Keeps the THSR booking flow in Rust.
- CAPTCHA is **not solved automatically**. The user enters it manually.
- CAPTCHA image is served from the running program in a browser-friendly page.
- On a local desktop, the CAPTCHA page is opened automatically when possible.
- On Railway, the CAPTCHA URL uses `RAILWAY_PUBLIC_DOMAIN` (or `THSR_PUBLIC_URL`) instead of `127.0.0.1`.

## Railway setup

1. Deploy this repository as a Railway service.
2. In **Settings -> Networking -> Public Networking**, click **Generate Domain**. Railway documents that this creates a public `*.up.railway.app` domain, and `RAILWAY_PUBLIC_DOMAIN` is then available to the service. 
3. Set these variables:

- `THSR_FROM=2` (Taipei)
- `THSR_TO=12` (Zuoying)
- `THSR_DATE=2026/09/20`
- `THSR_TIME=27` (18:00; use `--list-time-table` to confirm)
- `THSR_ADULT_CNT=1`
- `THSR_PERSONAL_ID=YOUR_ID`
- `THSR_RETRY_SECONDS=3`
- `THSR_SEAT_PREFER=0`
- `THSR_CLASS_TYPE=0`
- `THSR_USE_MEMBERSHIP=false`

`PORT` is supplied by Railway and is used by the CAPTCHA web server.

When a CAPTCHA is needed, the log will print a URL like:

`https://YOUR-SERVICE.up.railway.app/captcha/<token>/`

Open that URL, enter the CAPTCHA, and press **送出**. The Rust process then continues the booking flow.

If Railway has no public domain, the program will no longer print a misleading `127.0.0.1` URL. It will explicitly tell you to generate a domain or set `THSR_PUBLIC_URL`.

## Local

```bash
cargo run -- --from 2 --to 12 --date 2026/09/20 --time 27 --adult-cnt 1 --personal-id YOUR_ID --retry-seconds 3
```

## Build

```bash
cargo build --release
```

## Important

The CAPTCHA remains a manual step. This project does not attempt to bypass or automatically solve the site's CAPTCHA.
