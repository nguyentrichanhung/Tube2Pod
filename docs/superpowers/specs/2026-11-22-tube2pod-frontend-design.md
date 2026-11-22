# Tube2Pod · Thiết kế frontend máy chủ và app điện thoại

**22/11/2026 · Bản thiết kế v2 chờ duyệt**

Tube2Pod có hai giao diện dùng chung dữ liệu trên máy ở nhà. Website phục vụ việc lấy toàn bộ audio từ YouTube, quản lý thư viện và cấp quyền điện thoại. App phục vụ việc nghe, theo dõi show, chỉnh thông tin và phát hành các tập đã được tạo. [Kiến trúc đã duyệt](2026-11-22-local-first-mobile-design.md)

## Hướng thiết kế

- **Website máy chủ.** Điều hướng ngang cạnh nhận diện Tube2Pod. Nội dung căn giữa trên màn rộng. Khu ghép điện thoại chiếm toàn chiều rộng, hướng dẫn và nút tạo mã ở trái, QR ở phải. Danh sách thiết bị và truy cập ngoài nhà nằm ở hàng dưới.
- **App điện thoại.** Các mục **Nghe**, **Show** và **Quản trị** nằm ngay dưới tên app. Nút **Kết nối** hiển thị đường kết nối khi app đã ghép. Màn Nghe đặt bìa vuông bên cạnh tên tập, thời lượng và nút phát, với danh sách tập ngay dưới. Khi có tập được chọn, trình phát thu gọn nằm riêng ở đáy màn và mở lại được từ tên tập.
- **Màu và kiểu chữ.** Nền than `#13141B`, bề mặt `#1C1E28`, vùng ghép cặp và tập nổi bật màu chàm `#292842`, nút chính và mục đang chọn màu hổ phách `#FFC077`. Dùng sans-serif có dấu tiếng Việt và biểu tượng sóng ba vạch Tube2Pod. Trạng thái luôn đi cùng nhãn. Ảnh bìa dùng ảnh do người dùng cung cấp hoặc bìa mặc định của show. [Ảnh mẫu Alpine Mist](https://unsplash.com/photos/a-mountain-covered-in-trees-and-fog-uCcCAf6hGQU)
- **Nhịp bố cục.** Khoảng cách theo nhịp 4, 8, 12, 16, 24 và 32. Trên màn hẹp, các vùng web thu về một cột và sáu mục điều hướng chia thành hai hàng. App dùng vùng chạm tối thiểu 44 pt trên iOS và 48 dp trên Android, tôn trọng vùng an toàn và cỡ chữ hệ thống.
- **Phản hồi tại chỗ.** Viền focus rõ, lỗi trường đặt dưới trường, cập nhật hàng đợi có thông báo hỗ trợ trình đọc màn hình. Khi đang tải hoặc lưu, giữ nội dung đã có và hiển thị tiến trình ở vùng thao tác. Hỗ trợ giảm chuyển động. Các hành động xóa, thu hồi và đổi liên kết giữ thông tin xác nhận ở ngay nơi người dùng thao tác.

## Website máy chủ

| Trang | Bố cục và thao tác |
| --- | --- |
| **Kết nối** | Khu ghép cặp chứa lời dẫn mở app, QR dùng một lần, thời gian còn lại và nút tạo mã mới. Danh sách thiết bị cho biết quyền từng máy và cho thu hồi từng thiết bị. **Truy cập ngoài nhà** hiển thị bật, đang kết nối, khả dụng hoặc lỗi, cùng địa chỉ HTTPS khi đã cấp. |
| **Tạo tập** | Dán link YouTube và chọn show trước khi xem trước. Vùng xem trước toàn chiều rộng đặt thông tin video cạnh các bản audio. Bản mặc định đứng đầu hoặc dùng **Đề xuất** khi nguồn thiếu cờ. Hiện ngôn ngữ YouTube của bản mặc định, dùng ngôn ngữ mặc định của show khi thiếu nhãn. Bản đi kèm bắt đầu ở **Chưa rõ ngôn ngữ** và cho chỉnh. Tạo tập đưa lựa chọn vào hàng đợi. Hàng dưới gồm hàng đợi và tập gần đây, có đường dẫn Thư viện. |
| **Thư viện** | Tìm theo video hoặc kênh, lọc theo show và trạng thái. Mỗi hàng có tên, show, thời lượng nếu có và trạng thái. Giữ phân trang, chọn nhiều và xác nhận xóa theo luồng hiện tại. |
| **Chi tiết tập** | Tiêu đề nguồn, show, ngôn ngữ và trạng thái nằm ở đầu. MP3 và mô tả nguồn nằm ở vùng nội dung, cùng bản tóm tắt của tập cũ nếu đã có. Thao tác phát hành, hủy, thử lại hoặc xóa phụ thuộc trạng thái hiện có. |
| **Show** | Lưới bìa hiển thị tên show, ngôn ngữ mặc định, số tập và trạng thái RSS. Form tạo show đặt cạnh lưới khi đủ chỗ, gồm tên, mô tả, danh mục và ngôn ngữ. |
| **Chi tiết show** | Ba mục **Tập**, **Thông tin**, **Feed RSS**. Tập dẫn tới chi tiết và Thư viện đã lọc. Thông tin giữ biểu mẫu sửa, ảnh bìa và xác nhận xóa theo tên show. Feed hiển thị điều kiện còn thiếu, bật hoặc tắt RSS, sao chép hoặc mở URL, phát hành tập sẵn sàng, đổi link theo xác nhận hiện có và đường dẫn Apple Podcasts Connect. |
| **Hoạt động** | Hàng đợi và lỗi gần đây, mỗi hàng dẫn tới tập để xem chi tiết và thử lại. |
| **Cài đặt** | Ba hàng chủ sở hữu, mặc định phát hành và giới hạn nguồn. Trên màn rộng, tên nhóm ở trái, trường cấu hình ở phải. |

Website vẫn dùng các trang và form của Axum/Askama. Đề xuất đặt **Kết nối** trong điều hướng chính và dẫn chủ máy mới tới đó từ bước mở website sau cài đặt. Đích điều hướng thường ngày và đường dẫn đã lưu vẫn hoạt động theo router hiện tại.

## App Android và iOS

| Màn hình | Bố cục và thao tác |
| --- | --- |
| **Kết nối** | Lần đầu dẫn vào ghép máy chủ tại nhà. Hướng dẫn mở QR trên website, xin quyền máy ảnh và mạng nội bộ tại bước cần dùng. Hiển thị **Trong mạng nhà**, **Từ xa** hoặc **Mất kết nối** theo đường tới máy chủ. Tùy tình huống, app cho quét lại, yêu cầu mã mới, mở quyền hệ điều hành hoặc thử kết nối lại. |
| **Nghe** | Tập đã phát hành và danh sách các tập khác khả dụng từ máy chủ. Mở chi tiết tập hoặc phát từ hàng. Trình phát có tên tập, ảnh bìa, vị trí phát, thời lượng, phát hoặc tạm dừng và mô tả nếu có. |
| **Show** | Lưới bìa hai cột cho các show người nghe có thể theo dõi. Chi tiết show hiển thị thông tin, trạng thái theo dõi và danh sách tập đã phát hành. |
| **Quản trị** | Thiết bị chưa ghép thấy lời hướng dẫn và nút mở Kết nối. Sau ghép cặp, danh sách show cho biết số tập sẵn sàng. Chi tiết đặt bìa, tên show và nút sửa ở đầu, tiếp theo là các tập sẵn sàng để phát hành và khu bật, tắt hoặc chia sẻ RSS. |
| **Tạo hoặc sửa show** | Biểu mẫu một cột cho tên, mô tả, ngôn ngữ mặc định, danh mục, tác giả và ảnh bìa. Lỗi lưu giữ form tại chỗ. Khi lưu thành công, dữ liệu hiển thị theo phản hồi máy chủ. |

**Đã phát hành** thuộc về tập. **Bật RSS** thuộc về show. **Truy cập ngoài nhà** thuộc về kết nối máy chủ. Chẳng hạn, một tập đã phát hành vẫn có thể nghe từ danh sách của máy đã ghép cặp trong LAN khi tạm tắt relay. Khả năng sao chép feed chia sẻ phụ thuộc URL HTTPS công khai đang dùng và feed đã bật. App đọc lại trạng thái tập sau khi mất kết nối giữa chừng lúc phát hành.

## Cấu trúc frontend đề xuất

| Vùng | Thành phần và trách nhiệm |
| --- | --- |
| **Khung web** | `templates/base.html` chứa nhận diện, điều hướng, vùng nội dung và liên kết bỏ qua điều hướng. CSS hiện có giữ kiểu dùng chung cho biểu mẫu, trạng thái, hàng nội dung, nút và panel. Các mẫu chia sẻ dùng cho những thành phần có cùng hành vi. |
| **Trang web** | Giữ `studio.html`, `library.html`, `episode.html`, `shows.html`, `show_detail.html`, `observability.html`, `settings.html`. Trang Kết nối gom ghép cặp, thiết bị và relay. Mỗi trang là đích điều hướng độc lập và dùng tiêu đề riêng. |
| **Cập nhật web** | Các vùng xem trước nguồn, hàng đợi, dòng thư viện và tiến trình tập tiếp tục là fragment HTMX. Form gửi qua các handler hiện có cho hành động hiện có. Phản hồi lỗi xuất hiện ở vùng của yêu cầu. Lần tải mới và trang sau bộ lọc vẫn hiển thị đủ bằng Askama. |
| **Khung Flutter** | `lib/app/` chứa khung điều hướng, theme và điểm vào ứng dụng. `lib/features/connection/`, `listen/`, `shows/` và `admin/` chứa màn hình và trạng thái hiển thị của từng luồng. `lib/shared/ui/` chứa các thành phần thật sự dùng chung, như hàng tập, bìa, trạng thái và khung biểu mẫu. |
| **Dữ liệu app** | Luồng nghe dùng RSS/enclosure hoặc danh sách đã phát hành của máy đã ghép cặp. Luồng quản trị dùng các khả năng của API đã duyệt cho show, bìa, feed và phát hành. Màn hình tiêu thụ trạng thái kết nối, quyền và kết quả thao tác từ lớp dữ liệu. Phần tích hợp nền tảng xử lý camera, khám phá LAN, xác minh định danh đã ghép cặp và phát media. |
| **Nguồn trạng thái** | SQLite và phản hồi từ máy ở nhà quyết định show, tập, quyền thiết bị và phát hành. App giữ trạng thái điều hướng và trình phát. Đường công khai và đường quản trị dùng ngữ cảnh kết nối tương ứng theo kiến trúc đã duyệt. |

## Trạng thái cần thể hiện khi duyệt

- **Ghép cặp:** chưa tạo mã, mã còn hạn, đã dùng, hết hạn, quyền hệ điều hành bị từ chối, xác minh máy chủ thất bại và thiết bị bị thu hồi. Thu hồi một thiết bị hiển thị đúng trên hàng đó và trong app tương ứng.
- **Mạng:** cùng Wi-Fi, chuyển sang đường từ xa, mất đường tới máy, và kết nối lại. Lỗi quản trị hiển thị trong khu Quản trị. Feed công khai vẫn có thể nghe qua đường dành cho người nghe khi đường đó còn hoạt động.
- **Nguồn và tập:** chưa có show để tạo tập, URL không hợp lệ, xem trước hoặc tải nguồn thất bại, chờ xử lý, đang xử lý, sẵn sàng, đã phát hành và đã hủy. Lỗi trả lại tác vụ có hành động tương ứng ở web.
- **Show và feed:** danh sách trống, ảnh bìa lỗi, dữ liệu feed còn thiếu, URL chưa khả dụng, tập đã phát hành và kết quả sao chép. Web và app dùng cùng thuật ngữ.

Bản xem thử Visual Companion dùng dữ liệu minh họa và có hai bề mặt dùng chung trạng thái trong phiên trình duyệt. Có thể duyệt bố cục và luồng chính từ tạo mã, ghép cặp, quản lý show, phát hành đến xem tập trong màn Nghe. Giao diện mô phỏng điều khiển phát và chuyển mạng. Các điều kiện nghiệm thu trên máy chủ, Android và iOS nằm trong tài liệu kiến trúc.
