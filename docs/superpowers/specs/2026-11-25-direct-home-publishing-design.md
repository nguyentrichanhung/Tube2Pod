# Tube2Pod — publish trực tiếp từ nhà, bộ cài đa nền tảng

Ngày: 25/11/2026. Trạng thái: phương án và layout đã duyệt; tài liệu tổng thể chờ duyệt.

Builder check độc lập đã hoàn tất: không còn câu hỏi quan trọng cản trở lập kế hoạch. Đây là review thiết kế, không phải bằng chứng phần mềm đã chạy trên các OS hay qua NAT.

## 1. Ý định và nguồn chuẩn

Người dùng chạy Tube2Pod trên máy tại nhà, quản trị bằng trình duyệt, đưa podcast ra Internet qua IP public và NAT trên router mình quản lý. Người ngoài chỉ nghe và chia sẻ podcast. Không cần Tailscale, máy chủ relay hay dịch vụ xử lý Omnivoice.

Cơ sở triển khai là `origin/main` đã có pipeline tải audio nguồn bằng yt-dlp, chuyển MP3 bằng ffmpeg, SQLite, RSS, giao diện Axum/Askama/HTMX và ghép app qua LAN. Checkout chính đang cũ và có thay đổi chưa commit; công việc mới nằm ở worktree riêng `/tmp/tube2pod-direct-home-20261125`, không ghi đè checkout đó.

Nguồn liên quan:

- [Thiết kế máy chủ và mobile trước đây](2026-11-22-local-first-mobile-design.md).
- [Thiết kế frontend trước đây](2026-11-22-tube2pod-frontend-design.md).
- README, `src/main.rs`, `src/config.rs`, `src/web/connection.rs`, `src/web/public.rs`, `deploy/` trên nhánh mới.
- [Bản mẫu layout v1 đã được người dùng duyệt](assets/direct-home-layout-v1.html). Bản mẫu chỉ là tài liệu hình ảnh/tương tác; dữ liệu và trạng thái minh họa không được đưa vào sản phẩm.

Yêu cầu mới thay thế lựa chọn relay làm đường truy cập ngoài nhà cho bộ cài này. Các tính năng mobile/LAN đã có không bị xóa. Hành vi nghiệp vụ không được thay đổi chỉ vì bản mẫu giản lược giao diện.

## 2. Những gì người dùng đã chọn

- Mạng nhà có IP public, có quyền cấu hình NAT.
- Ngoài nhà chỉ nghe và chia sẻ podcast, không quản trị.
- Chọn bộ cài server + web quản trị, không chọn desktop shell.
- Cần Windows, Linux và macOS/MacBook.
- Đã duyệt layout v1: điều hướng ngang, bề mặt trung tính, điểm nhấn hổ phách, sáng/tối/theo hệ thống; Kết nối, Tạo tập, Thư viện và Show/Feed RSS.
- Dùng `unslop` và `taste-skill` cho thiết kế; giữ cấu trúc ứng dụng và nhiệm vụ người dùng làm chuẩn, không biến trang quản trị thành landing page.

## 3. Mục tiêu, giới hạn và điều không được xảy ra

### Mục tiêu

1. Có gói chạy server cho Windows x64, Linux x64, macOS Apple Silicon và macOS Intel. Đây là ma trận kiến trúc do người thiết kế đề xuất, chờ duyệt cùng tài liệu; phiên bản OS thực tế đã kiểm chứng phải ghi trong hướng dẫn phát hành.
2. Người dùng quản trị tại `http://localhost:8402`, tạo tập theo pipeline hiện có và chia sẻ feed HTTPS trên tên miền riêng.
3. Đường Internet chỉ phục vụ public router: feed, MP3, ảnh bìa và health endpoint hiện có. Không route tới trang quản trị, owner hoặc API mobile.
4. Giao diện thực dùng layout đã duyệt nhưng giữ đầy đủ hành vi hiện có.
5. Không cần cấu hình relay hoặc tài khoản Tailscale để cài mới và publish trực tiếp.

### Ngoài phạm vi

- Không viết desktop shell, trình phát công khai mới, app mobile mới hoặc thay pipeline audio.
- Không tự mở router bằng UPnP, tự mua tên miền, tự quản lý DNS động, hay dùng dịch vụ kiểm tra mạng bên thứ ba.
- Không thêm tài khoản quản trị Internet, cơ chế đăng nhập mới hoặc tunnel mới.
- Không xóa relay khỏi mã nguồn hay tự chuyển các triển khai đang dùng relay sang địa chỉ khác.
- Không phát hành lên store, không tự cập nhật phần mềm, không thêm bộ cài GUI hay dịch vụ nền Windows/macOS trong lượt này.

Không được coi kiểm tra localhost thành bằng chứng truy cập từ Internet; không được tự đổi feed URL/token, xóa dữ liệu hoặc khởi động phiên bản mới trên dữ liệu thật để thử giao diện.

## 4. Phương án đã xem xét

| Phương án | Lợi ích | Đánh đổi | Quyết định |
| --- | --- | --- | --- |
| Server Rust + giao diện web hiện có | Tái sử dụng pipeline, dữ liệu, routes và UI; chỉ cần đóng gói cùng cấu hình HTTPS | Quản trị bằng trình duyệt, cần công cụ media và cấu hình router | Người dùng chọn phương án 1 |
| Desktop shell bọc server/web | Có cửa sổ ứng dụng riêng | Thêm dependency, lifecycle backend và đóng gói; không giải quyết NAT tốt hơn | Không chọn |

Caddy trên chính máy nhà là lựa chọn kỹ thuật đề xuất cho HTTPS. Không tích hợp một máy chủ TLS/public ingress mới vào Rust khi public router đã có.

## 5. Kết nối và HTTPS

Luồng: ứng dụng podcast của người nghe → tên miền HTTPS → router NAT → Caddy trên máy nhà → `127.0.0.1:8401`.

- Dùng subdomain riêng, ví dụ minh họa `podcast.example.com`. Chủ máy cung cấp tên miền thật khi triển khai. DNS trỏ tới IP public; nếu IP đổi, chủ máy cập nhật DNS bằng phương thức mình quản lý.
- Cấu hình mẫu Caddy kết thúc HTTPS và reverse proxy duy nhất tới public listener. Public origin được đặt trong `PUBLIC_BASE_URL` theo cơ chế hiện có; không thêm form ghi cấu hình từ web trong lượt này.
- Hướng dẫn chuẩn dùng NAT TCP 80/443 tới Caddy để cấp chứng chỉ và phục vụ HTTPS; router và firewall do chủ máy cấu hình. Không mở 8400, 8401, 8402 hoặc 8403 ra Internet. Chứng chỉ và DNS là hạ tầng Internet thông thường; nội dung podcast và luồng audio không qua máy chủ relay bên thứ ba.
- Admin `8400`, public `8401` và owner `8402` tiếp tục bind loopback. LAN `8403` phục vụ điện thoại theo quyền ghép cặp hiện có, không đưa qua Caddy/NAT.
- Chưa có `PUBLIC_BASE_URL`: vẫn dùng localhost/LAN được, UI giải thích chưa có địa chỉ chia sẻ Internet. Có URL hợp lệ: ghi “Đã cấu hình”, kèm “Chưa kiểm tra truy cập từ Internet”; không báo “Online” hay “Đã xác minh” bằng suy đoán.
- Không thêm nút kiểm tra tự động hoặc lưu trạng thái xác minh Internet. Hướng dẫn người dùng mở feed, ảnh và audio bằng 4G/5G hoặc mạng ngoài nhà để kiểm chứng. Nếu mạng nhà không hỗ trợ NAT loopback, việc URL public không mở được từ chính Wi-Fi không tự kết luận Internet bị lỗi.
- RSS, HEAD, byte range/seek, content type, cache headers và điều kiện feed/audio/cover tiếp tục dùng public router hiện tại.
- Tắt relay trong cấu hình mẫu mới bằng cách không khai báo endpoint. Với triển khai có relay từ trước, giữ khả năng đó và hiển thị riêng, không làm trạng thái relay quyết định trạng thái direct HTTPS.

Tài liệu tham khảo kỹ thuật: [Caddy automatic HTTPS](https://caddyserver.com/docs/automatic-https), [reverse proxy](https://caddyserver.com/docs/quick-starts/reverse-proxy).

## 6. Bộ cài và vòng đời server — đề xuất cần duyệt

Để giữ phạm vi nhỏ, “bộ cài” ở lượt này là **gói giải nén chứa binary Tube2Pod, cấu hình mẫu, script chạy và hướng dẫn**, không phải MSI/DMG có wizard.

| Nền tảng | Gói đề xuất | Cách chạy mới |
| --- | --- | --- |
| Windows x64 | ZIP, binary native, launcher PowerShell | Chạy launcher trong terminal, giữ terminal để server hoạt động |
| Linux x64 | tar.gz, binary native, launcher shell | Chạy foreground; tiếp tục có installer systemd hiện hữu cho máy cần chạy nền |
| macOS arm64/x64 | tar.gz riêng từng kiến trúc, launcher shell | Chạy launcher trong Terminal, giữ terminal để server hoạt động |

- Launcher nạp file cấu hình theo cú pháp KEY=VALUE được tài liệu hóa, không thực thi nó như shell script. Dùng đường dẫn có khoảng trắng an toàn, báo rõ công cụ thiếu, lỗi cấu hình hoặc cổng đang bị chiếm.
- `yt-dlp`, `ffmpeg`, `ffprobe` và Caddy được người dùng cài trước theo hướng dẫn phù hợp OS, hoặc trỏ đường dẫn tới bản đã có. Gói không âm thầm tải/cài executable và không nhúng lại binary bên thứ ba. Không cần Rust trên máy sử dụng gói đã build.
- Launcher chạy Tube2Pod; Caddy chạy bằng lệnh foreground riêng hoặc dịch vụ Caddy do chủ máy đã cấu hình. Hướng dẫn trình bày rõ hai tiến trình cần chạy để chia sẻ HTTPS. Không tạo wrapper quản lý cả hai tiến trình.
- Đóng tab trình duyệt không dừng server. Dừng tiến trình server bằng Ctrl+C làm podcast ngừng phục vụ; không có tự khởi động lại/khởi động cùng máy mới trên Windows/macOS. Linux systemd giữ lifecycle hiện có.
- Không tự mở trình duyệt, thay cấu hình sleep, firewall, router hoặc quyền hệ thống. Sau khi chạy, in địa chỉ owner để người dùng mở. Máy ngủ, tắt hoặc mất mạng thì podcast không truy cập được.
- Dữ liệu nằm ở thư mục bền vững của người dùng, tách khỏi thư mục giải nén phiên bản. Đề xuất: `%LOCALAPPDATA%/Tube2Pod` trên Windows, `~/Library/Application Support/Tube2Pod` trên macOS, `${XDG_DATA_HOME:-~/.local/share}/tube2pod` cho launcher Linux. Installer systemd tiếp tục dùng `/var/lib/tube2pod`. `DATA_DIR` đã cấu hình luôn được tôn trọng; không tự di chuyển dữ liệu.
- Quy trình build tạo artifact cho bốn target bằng công cụ hiện có và native runner thích hợp. Không tuyên bố hỗ trợ một target chỉ vì đã cross-compile. Ký/notarize installer không thuộc phạm vi gói giải nén này; không hướng dẫn tắt bảo vệ toàn hệ điều hành.

## 7. Thiết kế UI đã duyệt và ánh xạ vào sản phẩm

### Hướng hình ảnh

- Sản phẩm: công cụ tạo và publish podcast từ máy nhà.
- Tính cách: điềm tĩnh, rõ ràng, gần gũi.
- Chất liệu: thư viện audio cá nhân; nền trung tính, bề mặt nhẹ, chữ dễ đọc.
- Điểm nhấn: bìa show, thông tin tập và tiến trình thực; không dùng số liệu trang trí.
- Dấu hiệu riêng: logo ba vạch sóng hiện có, điểm nhấn hổ phách có tiết chế.

Giữ điều hướng ngang, responsive một cột trên màn hẹp, trạng thái chọn trang rõ; font hệ thống hỗ trợ tiếng Việt. Dùng sáng/tối/theo hệ thống và cơ chế ghi nhớ hiện tại. Giữ focus, skip link, nhãn trường, thông báo lỗi, giảm chuyển động. Không thay đổi trang hiện tại hay dữ liệu form khi đổi theme.

### Các màn hình

| Màn hình | Thay đổi theo mẫu | Hành vi cần giữ |
| --- | --- | --- |
| Kết nối | Địa chỉ chia sẻ + sơ đồ máy nhà/router/người nghe; phạm vi public cạnh bên; ghép điện thoại bên dưới | QR thật, hết hạn/đã dùng, đổi tên/thu hồi thiết bị; relay nếu triển khai đã cấu hình được đặt riêng và gọi đúng tên |
| Tạo tập | Nguồn video và show, preview, lựa chọn audio, CTA; hàng đợi và tập gần đây phía dưới | HTMX xem trước, danh sách audio thật, ngôn ngữ, lỗi, lựa chọn người dùng, tiến trình và trạng thái backend |
| Thư viện | Bộ lọc và hàng nội dung dễ đọc; bìa/trạng thái/thời lượng nhất quán | Tìm/lọc/phân trang, chọn nhiều/xóa có xác nhận, mở chi tiết tập |
| Show/Feed RSS | Thông tin show, URL để sao chép, nội dung feed và điều kiện publish | Các tab Tập/Thông tin/Feed, bật tắt RSS, phát hành, đổi token có xác nhận, chỉnh bìa/thông tin/xóa theo quy tắc cũ |
| Chi tiết tập, Show list, Cài đặt, Hoạt động | Áp dụng cùng kiểu chữ, khoảng cách, màu và controls | Giữ routes, nội dung, thao tác và điều kiện nghiệp vụ hiện có |

Bản mẫu chỉ có bốn đích để duyệt, không phải yêu cầu bỏ các mục khác khỏi điều hướng. Các tab giả, dữ liệu mẫu, mô phỏng hàng đợi, URL `.example`, nhãn “Bản mẫu” và khối thay QR không đi vào sản phẩm. Tên miền public là thông tin thật từ cấu hình, không phải ID nội bộ cần người dùng copy.

Ảnh bìa dùng upload/fallback hiện có. Trang Kết nối không khẳng định HTTPS đã truy cập được. Trang Feed không tự thay URL hiện có và không đồng nhất “Đã phát hành”, “Bật RSS” với “Có Internet”.

## 8. Tương thích và dữ liệu

- Không yêu cầu migration/schema mới cho cấu hình public hay giao diện; đọc nguồn dữ liệu/config hiện có.
- Giữ feed token, episode UUID/GUID, thư mục dữ liệu, audio đã xuất, thông tin show và quyền thiết bị.
- Không ghi đè `.env`/cấu hình người dùng khi cài lại. Không tự thay public origin; nếu chủ máy chọn tên miền khác, giải thích ảnh hưởng URL đang được subscribe và chỉ triển khai redirect khi địa chỉ cũ còn thuộc quyền kiểm soát.
- README hiện có cảnh báo migration từ pipeline Omnivoice cũ sẽ dọn queued/running/review. Không sửa chính sách migration trong scope này. Thử nâng cấp bằng bản sao dữ liệu trước, không dùng dữ liệu thật của checkout cũ để demo.
- Chỉ sửa những chỗ phụ thuộc Unix cần thiết cho build/test/runtime đa nền tảng. Không bỏ test có ý nghĩa chỉ để báo Windows xanh; cần kiểm tra runtime tương ứng trên Windows cho thao tác media, đường dẫn và cancellation liên quan.

## 9. Phạm vi thay đổi dự kiến

- `deploy/`, hướng dẫn README và build/packaging scripts/workflow cho các target; config mẫu Caddy và native launchers.
- `src/web/connection.rs`, template Kết nối và tests để phản ánh direct public origin độc lập relay.
- `assets/app.css`, templates và fragments liên quan để áp dụng layout; giữ Axum/Askama/HTMX, không thêm framework frontend.
- Config/startup/tool path handling và các test/fixture phụ thuộc Unix chỉ khi cần cho launcher và các OS đã chọn.
- Không sửa app Flutter, relay protocol, pipeline nghiệp vụ hay hợp đồng public URL ngoài những gì tài liệu nêu.

## 10. Xác minh và điều kiện hoàn thành

1. Test Rust phù hợp thay đổi, format và diff checks. Config chấp nhận HTTPS riêng, vẫn chạy local khi chưa có public origin, không đòi Omnivoice/Tailscale.
2. Test trang Kết nối phân biệt chưa cấu hình/direct URL/relay; config thật được escape; QR và quản lý thiết bị không bị mất.
3. Kiểm tra UI thật với dữ liệu tách biệt: navigation, sáng/tối/hệ thống, source preview, queue, lỗi, lọc/phân trang, feed URL/copy và hành động bị ảnh hưởng. Kiểm tra màn rộng và 390px, keyboard/focus, không tràn ngang. Phân biệt kiểm tra mockup đã làm với kiểm tra UI sản phẩm.
4. Trên từng target native: giải nén gói, cài công cụ, khởi động với data dir có khoảng trắng, mở owner page, xử lý một nguồn có quyền dùng, phát MP3 và publish feed. Kiểm tra lỗi thiếu công cụ/cổng bận, dừng và mở lại với dữ liệu vẫn còn. Ghi rõ OS và kiến trúc nào thực sự đã chạy; build đơn lẻ không thay kết quả runtime.
5. Caddy public origin phục vụ RSS, ảnh và MP3 với HEAD và Range. Các đường quản trị/owner/API mobile không được truy cập qua public origin. Feed token/URL nghiệp vụ cũ được giữ theo cấu hình chủ máy.
6. Chỉ kết luận publish trực tiếp tại nhà đã đạt sau khi thử từ mạng ngoài nhà: DNS/HTTPS hợp lệ, feed tải được, app podcast đọc enclosure và phát/seek MP3. Kiểm tra từ localhost hoặc Wi-Fi cùng nhà không đủ. Nếu chưa có tên miền/router hoặc thiết bị ngoài mạng để chạy, báo chính xác phần còn thiếu thay vì tuyên bố hoàn tất.
7. Review độc lập phần packaging/network/UI trước bàn giao theo hướng dẫn dự án; không thêm vòng kiểm định ngoài scope.

## 11. Phần giao người triển khai và điểm cần duyệt

Người triển khai tự chọn tên script, tổ chức CSS, cách tái dùng template, runner build, test helper và chi tiết quoting/path theo cấu trúc repo. Không tự thay các quyết định sản phẩm trên.

Ngoài phương án và layout đã được duyệt, các đề xuất cần duyệt trong tài liệu này là: ma trận bốn target; gói giải nén, công cụ ngoài cài sẵn; foreground launchers và Caddy chạy riêng; không thêm tự chạy nền Windows/macOS; direct HTTPS qua Caddy; giữ hành vi nghiệp vụ và tính năng LAN hiện có.

Sau khi tài liệu được duyệt, bước tiếp theo là `superpowers:writing-plans`, rồi triển khai theo kế hoạch. Việc duyệt layout riêng chưa được xem là duyệt toàn bộ cách đóng gói/vận hành.
