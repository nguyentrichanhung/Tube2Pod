# Bộ cài Tube2Pod cho máy cá nhân

## Mục tiêu

Người mới tải tệp dành cho máy mình, mở bộ cài và chọn Cài đặt. Bộ cài tải các công cụ xử lý âm thanh qua Internet trong cùng lần thiết lập. Người dùng mở Tube2Pod từ danh sách ứng dụng và đưa một video vào show qua trình duyệt. Mọi thao tác thiết lập cho lần sử dụng cục bộ đều thực hiện được bằng chuột, kể cả khi máy chưa cài Rust, Python hoặc công cụ media.

## Các phương án đã cân nhắc

1. Bộ cài riêng theo hệ điều hành tạo biểu tượng ứng dụng. Người dùng chọn cách này vì giảm thao tác giải nén, tìm thư mục và chạy lệnh. Cần đóng gói và thử trên từng hệ điều hành.
2. Gói giải nén tự thiết lập tận dụng các launcher hiện có, nhưng người dùng phải tự chọn chỗ giữ gói và tìm tệp khởi chạy trong đó.

Người dùng đã chọn tải công cụ trong lần cài đầu. Nền triển khai là nhánh có sẵn quy trình đóng gói native, luồng dùng âm thanh nguồn và các mặc định dữ liệu portable.

## Phạm vi bản đầu và các lựa chọn đề xuất để duyệt

| Máy | Tệp người dùng mở | Điểm mở lại |
| --- | --- | --- |
| Windows x64 | Bộ cài .exe | Start |
| Ubuntu Desktop x64 | Gói .deb qua trình cài phần mềm đồ họa | Danh sách ứng dụng |
| macOS Apple Silicon và Intel | Bộ cài .pkg phù hợp kiến trúc | Applications |

Ubuntu Desktop là phạm vi Linux đề xuất cho lần phát hành đầu. Cùng một thiết kế có thể dùng phần hoàn tất thiết lập chạy với tài khoản người dùng sau khi hệ điều hành cài tệp ứng dụng. Nếu nền tảng cần người dùng bấm biểu tượng để hoàn tất, bộ cài chỉ rõ bước đó. Màn hình thiết lập tải công cụ trước khi đưa người dùng tới giao diện quản trị.

## Hành vi thiết lập và sử dụng

- Tải bản yt-dlp standalone và cặp ffmpeg, ffprobe phù hợp với hệ điều hành và CPU. Lấy qua HTTPS từ nguồn phát hành yt-dlp và nhà phân phối bản dựng được dự án FFmpeg liên kết. Người triển khai chọn phiên bản và xác minh cặp binary chạy được trên hệ điều hành mục tiêu trước khi đưa vào bộ cài.
- Cho biết công cụ đang tải. Tải vào vùng tạm và kiểm tra khả năng chạy của cả ba công cụ trước khi báo sẵn sàng. Nếu mất mạng hoặc thiếu chỗ, hiển thị nguyên nhân và nút **Thử lại**. Dùng lại công cụ đã tải thành công. Lần mở bình thường dùng bộ công cụ đã cài.
- Lưu công cụ trong vùng riêng của Tube2Pod và cấp đường dẫn cho cơ chế cấu hình hiện có. Với máy mới, dùng thư mục dữ liệu mặc định theo hệ điều hành. Bảo toàn cấu hình hiện có và dữ liệu khi cài lại hoặc gỡ ứng dụng. Đối tượng của bộ cài là máy cá nhân. Việc di chuyển một dịch vụ systemd hay cơ sở dữ liệu đời cũ tiếp tục theo hướng dẫn nâng cấp riêng.
- Đề xuất để duyệt: sau khi cài, người dùng chủ động bấm **Mở Tube2Pod**. Biểu tượng mở ứng dụng khởi chạy máy chủ cục bộ và mở trang chủ máy ở địa chỉ owner đang cấu hình khi máy chủ sẵn sàng. Mở lần nữa đưa người dùng về cùng máy chủ. Cung cấp thao tác **Dừng Tube2Pod** cho máy chủ do ứng dụng mở. Đóng tab tiếp tục cho phép nghe podcast như hiện tại. Việc cài đặt không đăng ký tự chạy khi đăng nhập.
- Phạm vi là cài và dùng cục bộ. Chia sẻ RSS qua Internet dùng hướng dẫn HTTPS hiện có. Thiết lập router, Caddy và thay đổi ứng dụng di động nằm ngoài bản thiết kế này.

## Đóng gói và xác minh để phát hành

Tận dụng workflow native và mã xử lý audio hiện có. Người triển khai quyết định công cụ tạo bộ cài, cách gọi giao diện thiết lập theo nền tảng và bố trí tệp nội bộ. Hướng dẫn tải chỉ rõ hệ điều hành, kiến trúc và tệp người dùng cần mở. Ghi rõ tình trạng chữ ký và lời nhắc thực tế của hệ điều hành sau khi thử gói.

Trên môi trường sạch cho mỗi mục tiêu, mở đúng tệp phát hành qua giao diện đồ họa, cài từ đầu và khởi chạy bằng biểu tượng. Xác nhận tải công cụ, vào được trang quản trị, xử lý audio nguồn được phép dùng thành MP3 và đọc được tập qua RSS. Kiểm tra mất mạng giữa lúc tải và thử lại, mở lần sau khi chặn nguồn tải công cụ, dừng máy chủ, cài lại và gỡ trên dữ liệu thử. Ghi kết quả riêng cho Windows x64, Ubuntu x64, macOS Apple Silicon và macOS Intel. Chỉ liệt kê gói đã tạo và nền tảng thực sự đã thử trong ghi chú phát hành.
