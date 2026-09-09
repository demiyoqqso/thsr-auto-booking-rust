# THSR Auto Booking (Rust)

Based on BreezeWhite/thsr-ticket-rs, modified for a headless Railway/container environment.

## What changed

- No Python required.
- Automatically selects the first available train.
- Automatically retries when no train is available.
- Fixes the previous broken `run_once` flow.
- Fixes the Linux/Railway CAPTCHA crash caused by `xdg-open`.
- CAPTCHA is shown through a small temporary web page instead of a desktop image viewer.
- Supports Railway environment variables for the booking parameters.

## Important

The CAPTCHA is intentionally left for the user to enter manually. The program does not attempt to bypass or solve the site's CAPTCHA.

## Railway variables

Set these variables in Railway:

- `THSR_FROM=2` (Taipei)
- `THSR_TO=12` (Zuoying)
- `THSR_DATE=2026/09/20`
- `THSR_TIME=27` (18:00; use `--list-time-table` to confirm IDs)
- `THSR_ADULT_CNT=1`
- `THSR_PERSONAL_ID=YOUR_ID`
- `THSR_RETRY_SECONDS=3`
- `THSR_SEAT_PREFER=0`
- `THSR_CLASS_TYPE=0`
- `THSR_USE_MEMBERSHIP=false`

Railway's `PORT` is used automatically. When a CAPTCHA is needed, the log prints a URL. Open that URL in your browser, enter the CAPTCHA, and submit it. The booking flow then continues automatically.

## Local

```bash
cargo run -- --from 2 --to 12 --date 2026/09/20 --time 25 --adult-cnt 1 --personal-id YOUR_ID --retry-seconds 3
```

## Build

```bash
cargo build --release
```
