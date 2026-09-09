# THSR Auto Booking — Fixed Settings V13

固定訂票條件：
- 出發：板橋
- 到達：台中
- 日期：2026/09/25
- 發車時間：07:30～12:00（中午12點，含12:00）
- 成人：2張
- 學生：0
- 座位：不限
- 車廂：標準車廂
- 會員：關閉
- 沒有符合條件的車次：3秒後重試
- CAPTCHA：由 Railway 網頁輸入，並立即把 CAPTCHA 網址傳到 Telegram
- 訂票成功：把 PNR Code 傳到 Telegram

## Railway Variables

必要：
- `THSR_PERSONAL_ID`
- `TELEGRAM_BOT_TOKEN`
- `TELEGRAM_CHAT_ID`

`THSR_PUBLIC_URL`：填 Railway 自己產生的公開網域，例如 `https://xxxx.up.railway.app`。

## 重要

這一版會在程式內強制固定 07:30～12:00，不能再被舊的 `THSR_TIME` 或其他舊設定覆蓋。

看到 `FIXED SETTINGS ... 07:30-12:00 NOON` 才代表這個版本正在執行。
看到 `AUTO SELECT` 時，發車時間必須落在 07:30～12:00；若超過 12:00，程式會拒絕訂票並重新嘗試。
