# Tube2Pod tại nhà và app Android/iOS

**22/11/2026 · Kiến trúc đã duyệt**

## Mục tiêu

Người dùng cài Tube2Pod Server trên máy ở nhà, mở trang quản trị và quét QR trong app điện thoại. Cùng mạng Wi-Fi, app mở được các podcast đã phát hành ngay từ máy chủ tại nhà. Chủ máy chủ có thể bật chuyển tiếp để nghe và quản trị từ xa qua Internet. Người dùng bình thường dùng địa chỉ do Tube2Pod cấp tự động. SQLite, nội dung audio, ảnh bìa và hàng đợi xử lý nằm trên máy ở nhà.

Website trên Tube2Pod Server tiếp tục nhận link YouTube, xem trước, chọn đúng bản audio và ngôn ngữ, chuyển toàn bộ bản đã chọn sang MP3 rồi xử lý các trạng thái của tập theo luồng đã duyệt. App điện thoại dùng dữ liệu do Tube2Pod Server cung cấp. App cho người nghe theo dõi RSS và phát MP3. Sau khi được cấp quyền quản trị, app còn tạo và chỉnh sửa show, cập nhật ảnh bìa, quản lý trạng thái feed và phát hành tập **Sẵn sàng** đã được tạo trên máy chủ.

Cơ sở thiết kế là bản đang chạy từ `bccac7f`. Axum đã tách listener quản trị và listener RSS/audio/cover, cả hai hiện bind loopback. `PUBLIC_BASE_URL` hiện bắt buộc là HTTPS và chứa địa chỉ tuyệt đối đưa vào RSS. Website dùng cookie chống CSRF và kiểm tra origin HTTPS. Tailscale hiện đứng trước hai listener. Luồng MP3 hiện có là nguồn chuẩn cho lựa chọn bản audio, giới hạn nguồn, trạng thái và cách phát hành. Thiết kế nguồn audio đã duyệt nằm trong `docs/superpowers/specs/2026-11-21-source-audio-podcast-design.md` của workspace chính.

## Phương án đã xem xét

| Phương án | Đánh đổi | Quyết định |
| --- | --- | --- |
| Tên miền riêng và Caddy trên từng máy nhà | Người dùng quản lý trực tiếp đường vào. Khâu DNS, chứng chỉ và router tạo ma sát khi cài đặt. | Giữ làm cấu hình nâng cao cho host đã có đường HTTPS. |
| Chỉ dùng Wi-Fi và QR | Cài đặt tại nhà đơn giản. Khi ra ngoài, app mất đường tới máy chủ và danh mục podcast công khai thiếu URL Internet ổn định. | Dùng làm kết nối ưu tiên khi ở nhà. |
| Wi-Fi và QR, thêm chuyển tiếp tùy chọn | Người dùng bắt đầu tại nhà và có thể mở truy cập ngoài nhà với tên HTTPS cấp sẵn. Đơn vị vận hành chịu chi phí dịch vụ, băng thông và trạng thái kết nối. | Người dùng đã chọn. |
| Điện thoại tự chuyển audio và xuất RSS lên host tệp | Cần làm lại pipeline media trên hai hệ điều hành và giải quyết quyền nguồn YouTube cho app. | Yêu cầu sau cùng chuyển việc tạo MP3 về Tube2Pod Server. |

## Cài đặt và kết nối

1. Chạy gói cài đặt Linux x86-64 dành cho máy ở nhà. Gói mở website quản trị tại máy chủ và kiểm tra các công cụ `yt-dlp`, `ffmpeg`, `ffprobe` theo đúng luồng hiện có. Chi tiết đóng gói công cụ phụ thuộc vào nền tảng và giấy phép phân phối.
2. Chủ máy chủ mở **Kết nối điện thoại** trên website chạy tại máy ở nhà và tạo QR quản trị. QR chứa địa chỉ LAN, nhận diện máy chủ và mã ghép cặp dùng một lần có hạn. App yêu cầu quyền mạng nội bộ theo hệ điều hành, đọc mã và xác nhận đã kết nối với máy chủ đó.
3. App lưu nhận diện ổn định của máy chủ. Tìm máy chủ trong Wi-Fi bằng Bonjour/mDNS giúp app kết nối lại sau khi địa chỉ LAN thay đổi. QR vẫn dùng được khi mạng cho phép kết nối trực tiếp nhưng chặn khám phá mDNS. Danh sách đã phát hành và tệp còn trên thiết bị có thể nghe trong LAN khi mất Internet. [Apple Bonjour](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy) · [Android Network Service Discovery](https://developer.android.com/reference/android/net/nsd/NsdManager)
4. Chủ máy chủ có thể bật **Truy cập ngoài nhà**. Máy chủ mở kết nối đi ra tới dịch vụ chuyển tiếp của Tube2Pod. Dịch vụ cấp địa chỉ HTTPS công khai ổn định cho podcast và địa chỉ chuyển tiếp riêng cho app quản trị. App nhận thông tin kết nối từ máy chủ đã xác minh và dùng đường chuyển tiếp khi kết nối LAN không khả dụng. App hiển thị trạng thái kết nối và cho thử lại khi máy nhà hoặc chuyển tiếp mất kết nối.
5. Người nghe dùng URL RSS được chủ show chia sẻ. Trong app đã ghép cặp, chủ máy chủ thấy các show của máy mình và mở được các tập đã phát hành trong LAN. Địa chỉ chia sẻ công khai được cấp từ cấu hình HTTPS có sẵn hoặc chuyển tiếp. Lần đầu bật chuyển tiếp cấp một địa chỉ công khai cho server. Bật lại hoặc kết nối lại cùng server dùng địa chỉ đã cấp.

## Quyền quản trị và chuyển tiếp

### Những gì người dùng đã chọn

- Chuyển tiếp do Tube2Pod vận hành là tùy chọn. Chủ máy chủ quyết định khi nào bật. Quyền quản trị từ xa đi cùng việc ghép cặp đã được cấp.
- QR cấp quyền quản trị dùng một lần. Mỗi thiết bị có quyền riêng và một hàng trong danh sách thiết bị của máy chủ. Chủ máy chủ thu hồi từng thiết bị trên website.
- RSS công khai, audio và ảnh bìa chia sẻ đi qua đường phục vụ người nghe. App quản trị dùng đường riêng tới API của máy chủ. Máy chủ ở nhà là nơi quyết định quyền và nội dung trả về. Quản trị từ xa được mã hóa đầu-cuối giữa app và máy ở nhà.

### Ranh giới kỹ thuật

- **Website tại máy chủ.** Chức năng ghép cặp và khôi phục quyền mở từ trình duyệt trên máy ở nhà. Dùng địa chỉ `http://localhost:<cổng cài đặt>` trên listener loopback dành riêng cho chủ máy. Kiểm tra `Host` và origin đúng địa chỉ đã cấu hình, giữ cookie và token chống CSRF cho các thao tác ghi, và kiểm chứng hành vi này trên các trình duyệt thuộc gói cài được chọn. Ngoại lệ loopback chỉ áp dụng cho listener chủ máy. HTTPS và kiểm tra origin hiện có tiếp tục áp dụng trên các triển khai website từ xa đã cấu hình riêng. Người có quyền vận hành máy cài Tube2Pod có quyền cấp lại và thu hồi từng thiết bị. [W3C Secure Contexts](https://www.w3.org/TR/secure-contexts/#localhost) · [Cookie Secure trên localhost](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Set-Cookie#secure)
- **Ticket ghép cặp.** Máy chủ tạo mã ngẫu nhiên có hạn ngắn. Trong một thao tác SQLite, máy chủ tiêu thụ mã và cấp thông tin đăng nhập riêng cho thiết bị. Lần quét thứ hai, mã hết hạn và mã bị hủy đều bị từ chối. App lưu bí mật trong kho bảo mật của hệ điều hành. Mọi lượt quản trị kiểm tra quyền ở máy chủ. Thu hồi có hiệu lực trên các yêu cầu tiếp theo, ở cả LAN lẫn Internet. Chủ máy có thể đặt tên thiết bị trên website để nhận diện khi thu hồi.
- **Kết nối LAN.** QR gắn kết nối với định danh TLS của máy chủ tại nhà. App xác minh định danh này khi gọi API và khi phát audio từ máy đó. mDNS cung cấp địa chỉ để kết nối. Pin của máy chủ không đổi vì địa chỉ IP thay đổi. Khi định danh máy chủ thực sự đổi, app yêu cầu ghép cặp lại. Cài đặt hệ điều hành của điện thoại và các hostname khác tiếp tục dùng quy tắc tin cậy thông thường.
- **Mã hóa quản trị từ xa.** Máy chủ xác thực kết nối đi ra với relay bằng thông tin đăng ký riêng. Relay dùng định danh định tuyến để chuyển luồng TLS quản trị tới máy ở nhà. Phiên TLS quản trị kết thúc ở app và máy ở nhà. Khóa riêng của máy chủ lưu tại máy nhà. App kiểm tra khóa công khai theo định danh nhận qua QR trước khi gửi thông tin đăng nhập của thiết bị hoặc dữ liệu quản trị. Nếu relay chuyển nhầm máy chủ hoặc app gặp định danh lạ, app dừng kết nối và báo lỗi. Relay có thể quan sát định danh định tuyến, thời điểm và dung lượng truyền. Nội dung yêu cầu, phản hồi và thông tin đăng nhập quản trị nằm trong phiên TLS đầu-cuối. Giao thức vận chuyển cụ thể do người triển khai chọn trong phạm vi này. Luồng xác minh định danh của từng host cần hoạt động trên Android và iOS với quy tắc tin cậy thông thường dành cho các kết nối còn lại của app. [TLS 1.3](https://www.rfc-editor.org/rfc/rfc8446) · [Apple Server Trust](https://developer.apple.com/documentation/foundation/performing-manual-server-trust-authentication)
- **Các đường đi qua relay.** Điện thoại dùng API quản trị từ xa sau ghép cặp. Website làm đầu mối cài đặt và thu hồi tại máy ở nhà. Relay có thể kết thúc HTTPS cho đường phục vụ người nghe, gồm RSS, MP3 đã phát hành và ảnh bìa của show được bật. Đường quản trị chỉ tới API của app qua kênh đầu-cuối. Thông tin đăng nhập thiết bị chỉ được gửi trên kênh quản trị. Người nghe truy cập đường RSS công khai bằng URL feed đã chia sẻ. SQLite và kho MP3 nằm trên máy ở nhà. Chủ show chủ động chia sẻ URL feed theo token hiện có.

## Luồng quản lý và phát hành

- Website và app dùng chung nghiệp vụ quản lý show và phát hành. API JSON có phiên bản dành cho tạo/sửa thông tin show, ngôn ngữ mặc định, bìa, cấu hình bật/tắt RSS, danh sách tập đã có, và phát hành tập **Sẵn sàng**. Các luồng xóa dữ liệu, xoay token feed, dán link YouTube và điều khiển xử lý tiếp tục thực hiện qua website theo quy tắc hiện có. Mỗi tập có một bản ghi trong SQLite của máy nhà.
- Trạng thái **Đã phát hành** của tập và nút **Bật RSS** của show tiếp tục là hai quyết định riêng. App có thể thấy tập đã phát hành từ máy chủ được ghép cặp. URL công khai chỉ có nội dung khi chủ máy có địa chỉ HTTPS công khai hoạt động và show đã bật RSS, đủ thông tin. Các chế độ tự động và thủ công dùng lại điều kiện hiện tại của máy chủ. Khi thử lại yêu cầu sau sự cố mạng, app đọc lại trạng thái tập từ server và giữ một định danh tập.
- App Android/iOS ưu tiên Flutter theo hướng đã đề xuất. Trình phát đọc MP3 từ nội dung của máy chủ hoặc URL enclosure đã chia sẻ. App khai báo rõ quyền mạng nội bộ. Tính khả thi được kiểm tra bằng kết nối và phát audio thực tế trên Android/iOS với chứng chỉ LAN gắn với định danh đã ghép cặp. Nội dung thử nghiệm và dữ liệu cấp cho người duyệt store dùng nguồn có quyền phân phối. Chuẩn bị máy chủ demo hoạt động độc lập và hướng dẫn người duyệt thao tác ghép cặp, quản lý show, phát MP3 và thử phát hành. Quyền truy cập đánh giá trên Google Play phải tái sử dụng được trong thời gian duyệt. Có thể dùng một URL demo ổn định để người duyệt lấy mã mới cho máy chủ demo theo từng lượt. [Flutter](https://docs.flutter.dev/reference/supported-platforms) · [Apple App Review](https://developer.apple.com/app-store/review/guidelines/) · [Google Play App Access](https://support.google.com/googleplay/android-developer/answer/15748846?hl=en)

## Tương thích và mất kết nối

- Tách địa chỉ công khai khỏi yêu cầu khởi động để cài mới trong LAN chạy trước khi bật relay. Đề xuất giữ `PUBLIC_BASE_URL` khi đã cấu hình hợp lệ. Kết nối nội bộ của app dùng địa chỉ nội bộ đã xác minh. RSS chia sẻ công khai dùng địa chỉ HTTPS ổn định đã chọn, phục vụ được các yêu cầu `HEAD` và byte range, với enclosure, ảnh bìa và GUID nguyên vẹn. [Apple Podcasts RSS](https://podcasters.apple.com/support/823-podcast-requirements)
- Giữ nguyên dữ liệu, token feed, ID tập và GUID của cài đặt hiện tại. Chủ máy quyết định lúc chuyển URL công khai. Nếu có người theo dõi feed cũ, chuyển hướng `301` từ URL cũ khi còn quyền quản lý URL đó. Kiểm tra bằng bản sao SQLite và MP3 trước khi đổi địa chỉ của dịch vụ đang chạy. [Apple hướng dẫn đổi RSS](https://podcasters.apple.com/support/837-change-the-rss-feed-url)
- Nếu tắt relay hoặc máy nhà tắt, người nghe ngoài nhà thấy kết nối không khả dụng cho tới khi máy chủ mở lại. Dữ liệu gốc vẫn tại máy. Điện thoại trong LAN giữ đường trực tiếp khi relay lỗi và máy nhà đang chạy. Tắt relay ngắt đường vào qua relay và không đổi trạng thái phát hành của tập trong SQLite.
- Dùng được một máy chủ với các app trên nhiều điện thoại đã cấp quyền. Đổi điện thoại thực hiện bằng QR mới. Mất điện thoại có thể thu hồi máy cũ từ website tại nhà. Đổi kết nối hoặc khởi động lại không thay đổi GUID hay địa chỉ cấp cho cùng một máy chủ đã đăng ký. Chủ máy giữ bản sao dữ liệu cùng thông tin nhận diện khi muốn khôi phục trên máy khác.

## Điều kiện nghiệm thu

1. Cài bản mới trên nền tảng máy chủ được duyệt. Mở website tại máy, tạo QR và kết nối ứng dụng Android/iOS trong cùng Wi-Fi. Phát một tập đã có và kết nối lại khi máy chủ đổi địa chỉ LAN.
2. Dùng cùng một QR cho hai thiết bị lần lượt và đồng thời. Chỉ một lượt ghép cặp được cấp quyền. Kiểm tra mã hết hạn. Cấp cho hai thiết bị bằng hai mã, thu hồi một thiết bị, xác nhận chỉ máy còn quyền có thể gọi API quản trị. Kiểm tra website dùng đúng origin và CSRF trên listener tại máy.
3. Bật relay trên mạng thử không có đường vào từ Internet. Dùng thiết bị Android/iOS qua dữ liệu di động để phát, sửa show và phát hành tập đã tạo. Kiểm tra phiên TLS quản trị kết thúc ở máy nhà và phần nội dung quản trị đi qua relay đã được mã hóa. Cho relay chuyển kết nối sang một máy Tube2Pod hợp lệ khác và xác nhận app từ chối khóa khác với mã QR đã quét trước khi gửi thông tin đăng nhập. Thử thu hồi quyền qua cả LAN lẫn relay. Khi kết nối lại đúng máy chủ, URL RSS giữ nguyên.
4. Từ ngoài mạng nhà, xác minh RSS, `HEAD`, `Range`, MP3 và ảnh bìa của feed đã bật. Tập còn **Sẵn sàng** và đường quản trị yêu cầu quyền không xuất hiện qua đường công khai. Ngắt riêng kênh quản trị trong khi máy ở nhà và đường công khai vẫn hoạt động. Kiểm tra người nghe vẫn truy cập được RSS, MP3 và ảnh bìa qua URL đã chia sẻ. Khi tắt toàn bộ relay, kiểm tra app báo đúng và tiếp tục nghe trong LAN.
5. Trên bản sao máy chủ hiện tại, kiểm tra toàn bộ luồng tạo tập từ YouTube qua website, phát hành qua app và phát qua RSS từ bên ngoài. Đối chiếu thời lượng MP3, ID, GUID, link feed và dữ liệu của các show có sẵn. Trình duyệt hoặc một app podcast thông thường phải đọc được feed Internet ngoài app Tube2Pod.

## Thiết kế frontend

Chủ máy quản lý ghép cặp và khôi phục quyền ngay trên website tại máy. App điện thoại được ghép cặp dùng quyền đã cấp khi quản trị từ xa. Bản thiết kế màn hình và cấu trúc frontend đang chờ duyệt. [Thiết kế frontend](2026-11-22-tube2pod-frontend-design.md)
