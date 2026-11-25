# Máy chủ Tube2Pod tại nhà

Gói giải nén chạy server foreground; quản trị bằng browser trên chính máy đó.
Mục tiêu: Windows x64, Linux x64, macOS Apple Silicon và Intel. Xem bảng kiểm chứng
trong README trước khi coi một nền tảng đã được thử thực tế.

## Công cụ cài riêng

- [yt-dlp chính thức](https://github.com/yt-dlp/yt-dlp#release-files): dùng
  `yt-dlp.exe` (Windows), `yt-dlp_macos` (macOS), `yt-dlp_linux` (Linux x64).
  Đổi tên theo ý bạn rồi đặt `YTDLP_PATH` là đường dẫn tuyệt đối. Trên Unix,
  cấp quyền chạy cho file đã tải bằng `chmod +x /đường/dẫn/yt-dlp`.
  Dùng bản standalone: entrypoint từ `pip install --user` có thể mất module
  Python khi ứng dụng chạy yt-dlp với HOME/cache riêng.
- [FFmpeg](https://ffmpeg.org/download.html): cài cả `ffmpeg` và `ffprobe`.
  Ubuntu/Debian: `sudo apt install ffmpeg`; macOS có thể dùng `brew install ffmpeg`.
  Windows dùng bản build được liên kết tại trang FFmpeg, giải nén và trỏ hai
  đường dẫn `.exe` trong cấu hình. Không cần thêm chúng vào PATH nếu đã trỏ đường dẫn.
- [Caddy chính thức](https://caddyserver.com/docs/install): tải binary phù hợp
  hệ điều hành/kiến trúc. Chỉ cần khi chia sẻ HTTPS; server local không đòi Caddy.

Kiểm tra `yt-dlp --version`, `ffmpeg -version`, `ffprobe -version`, `caddy version`
(hoặc dùng đường dẫn tuyệt đối tương ứng). Gói không chứa các công cụ này và
không tự tải chúng. Không cần Rust để sử dụng binary Tube2Pod đã build.

## Chạy server

Giải nén gói đúng kiến trúc. Copy `portable.env.example` thành `tube2pod.env`
và sửa đường dẫn công cụ nếu cần. Chưa có tên miền thì để PUBLIC_BASE_URL trống.

Linux/macOS, terminal thứ nhất:

```sh
./start.sh
# Hoặc file cấu hình khác (đường dẫn tương đối tính từ terminal hiện tại):
./start.sh '/đường dẫn/settings.env'
```

Windows, PowerShell trong thư mục giải nén:

```powershell
.\start.ps1
# Hoặc:
.\start.ps1 -Config 'C:\Tube2Pod config\settings.env'
```

Nếu chính sách máy chặn script, dùng binary trực tiếp với cùng cấu hình:
` .\tube2pod.exe --config 'C:\Tube2Pod config\settings.env' `.
Không cần tắt bảo vệ hệ điều hành. Sau khi chạy, mở địa chỉ owner được in ra
(mặc định `http://localhost:8402`). Đóng tab không dừng server; Ctrl+C trong
terminal dừng server. Máy ngủ/tắt hoặc mất mạng sẽ ngừng phục vụ podcast.
Không có tự khởi động cùng máy hoặc tự chạy nền Windows/macOS.

## Cấu hình và dữ liệu

File UTF-8 (BOM được chấp nhận), mỗi dòng `KEY=VALUE`; bỏ dòng trống và dòng
bắt đầu bằng `#`. Key gồm chữ ASCII, số và `_`, không bắt đầu bằng số.
Khoảng trắng ngoài key/value được bỏ; một cặp nháy đơn/đôi bao value được bỏ.
Không có nội suy biến, escape shell hoặc comment cuối dòng: `$HOME`, `~`,
`$(...)`, dấu `#` trong value đều là ký tự nguyên văn. Không dùng `export`.
Key trùng hay dòng sai gây lỗi có số dòng. Ví dụ:

```text
YTDLP_PATH=C:\Media Tools\yt-dlp.exe
FFMPEG_PATH=C:\Media Tools\ffmpeg.exe
FFPROBE_PATH=C:\Media Tools\ffprobe.exe
DATA_DIR=C:\Tube2Pod data
PUBLIC_BASE_URL=https://ten-ban.duckdns.org
```

Biến môi trường không rỗng ưu tiên hơn file, rồi mới đến mặc định. Chạy không
có `--config` giữ chế độ môi trường cũ; các mặc định portable chỉ áp dụng khi
có `--config`. Dùng đường dẫn tuyệt đối cho công cụ và DATA_DIR; launcher đổi
thư mục làm việc sang thư mục gói. Không ghi token/khóa thật vào cấu hình mẫu.

Khi không đặt DATA_DIR, dữ liệu portable nằm ngoài gói:

| OS | Thư mục |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Tube2Pod` |
| macOS | `~/Library/Application Support/Tube2Pod` |
| Linux | `$XDG_DATA_HOME/tube2pod` hoặc `~/.local/share/tube2pod` |

Nâng cấp: dừng server, sao lưu toàn bộ DATA_DIR và file cấu hình, giải nén gói
mới vào thư mục khác, dùng lại cấu hình và DATA_DIR cũ. Không xóa/đổi feed token
hay public URL. Khi nâng từ pipeline cũ, lần chạy đầu xóa các tập queued/running/review cũ
và file của chúng, kể cả audio đã phát hành đang được đọc lại. Các tập khác
giữ audio, transcript, summary và URL. Thử nâng cấp trên bản sao trước.

## Chia sẻ Internet

Làm theo [hướng dẫn HTTPS/NAT và DuckDNS miễn phí](direct-https.md). Username
của tài khoản trên máy là gợi ý tên miền phụ; phải còn trống và được tạo trong
tài khoản DuckDNS của bạn. DNS không relay nội dung audio.

Terminal thứ hai, sau khi cấu hình hostname trong bản sao `Caddyfile.example`:

```sh
caddy run --config Caddyfile --adapter caddyfile
```

Caddy chỉ proxy tới public `127.0.0.1:8401`. NAT TCP 80/443 tới Caddy; không đưa
8400/8401/8402/8403 ra Internet. Giữ cả hai tiến trình chạy. Kiểm tra bằng mạng
ngoài nhà: feed tải được trong app podcast, phát và tua MP3. Localhost không
chứng minh NAT/HTTPS Internet hoạt động.

## Linux systemd có sẵn

Gói Linux có `deploy/install-linux-x86_64.sh`, unit và cấu hình mẫu. Đọc README
nguồn cho điều kiện cài công cụ system-wide và backup, rồi chạy:
`sudo deploy/install-linux-x86_64.sh ./tube2pod`.
Installer giữ `/etc/tube2pod/tube2pod.env` và `/var/lib/tube2pod` hiện có, thay
binary/unit và restart. Đây là lựa chọn thay cho launcher foreground.
