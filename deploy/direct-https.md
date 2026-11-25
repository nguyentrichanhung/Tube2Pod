# Chia sẻ podcast trực tiếp qua HTTPS

Tube2Pod phục vụ podcast từ máy nhà qua Caddy: người nghe → tên miền HTTPS → router → Caddy → `127.0.0.1:8401`. Cài Caddy riêng theo [hướng dẫn chính thức](https://caddyserver.com/docs/install); launcher Tube2Pod không chạy Caddy. Máy phải hoạt động và kết nối mạng để phục vụ người nghe.

## Tên miền và DNS

Bạn cần IP public và quyền cấu hình router. Nếu chưa có tên miền, [Duck DNS](https://www.duckdns.org/why.jsp) cung cấp subdomain miễn phí. Gợi ý dùng tên đăng nhập trên máy cài đặt làm phần tên: `<username>.duckdns.org`. Xem tên người dùng hiện tại trong terminal trên máy đó:

- Windows PowerShell: `whoami` (dùng phần sau dấu `\`).
- Linux hoặc macOS: `whoami`.

Đăng nhập Duck DNS bằng tài khoản của chính bạn và tự tạo subdomain này nếu tên hợp lệ và còn trống; nếu không, chọn tên khác. Trên Duck DNS, đặt địa chỉ IP public của mạng nhà cho subdomain. Khi IP public đổi, tự cập nhật bản ghi bằng cách bạn quản lý; xem [đặc tả Duck DNS](https://www.duckdns.org/spec.jsp). Tube2Pod không đăng ký hay cập nhật DNS.

## Cấu hình và chạy

1. Sao chép `Caddyfile.example` ở thư mục gói (hoặc `deploy/Caddyfile.example` trong source checkout) thành file cấu hình Caddy của bạn và thay `podcast.example.com` bằng hostname thật. Proxy chỉ trỏ tới `127.0.0.1:8401`.
2. Trong cấu hình Tube2Pod, đặt `PUBLIC_BASE_URL=https://<hostname-thật>`. Để trống nếu chỉ dùng local/LAN. Khởi động lại Tube2Pod sau khi sửa cấu hình.
3. Trên router, chuyển tiếp TCP 80 và 443 tới máy chạy Caddy; cho phép các cổng đó qua firewall của máy. Không chuyển tiếp 8400, 8401, 8402 hay 8403 ra Internet. Giữ admin 8400, public 8401, owner 8402 trên loopback; 8403 chỉ dành cho ghép thiết bị LAN.
4. Chạy Tube2Pod và Caddy thành hai tiến trình riêng. Ví dụ chạy Caddy foreground với `caddy run --config /đường/dẫn/Caddyfile --adapter caddyfile`. Với hostname thật, DNS đúng và cổng mở, Caddy tự cấp và gia hạn chứng chỉ; xem [Automatic HTTPS](https://caddyserver.com/docs/automatic-https). Giữ dữ liệu Caddy bền vững để duy trì chứng chỉ.
5. Quản trị trên máy chủ tại `http://localhost:8402`. Ghép điện thoại trong mạng nhà qua LAN 8403 như trước.

Từ 4G/5G hoặc mạng ngoài nhà, mở feed của show đã bật RSS, kiểm tra ảnh bìa và phát/seek MP3 trong ứng dụng podcast. Nếu URL public không mở từ Wi-Fi nhà, router có thể không hỗ trợ NAT loopback; kiểm tra từ mạng ngoài trước khi kết luận. Kiểm tra cấu hình Caddy hoặc localhost không chứng minh truy cập Internet thành công.

Nếu đổi hostname, URL feed đang được đăng ký có thể thay đổi. Chủ máy quyết định có đổi hay không; chỉ thiết lập redirect từ địa chỉ cũ khi còn kiểm soát địa chỉ đó. Tube2Pod không tự chuyển feed hoặc token.
