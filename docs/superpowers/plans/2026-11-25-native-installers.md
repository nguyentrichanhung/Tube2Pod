# Kế hoạch triển khai bộ cài Tube2Pod

> Khi thực hiện, dùng superpowers:executing-plans cho triển khai trong phiên hiện tại hoặc superpowers:subagent-driven-development nếu người dùng chọn chia task cho worker.

**Mục tiêu.** Từ gói tải về trên máy sạch, người dùng có thể cài Tube2Pod, tự động nhận ba công cụ media và mở trang quản trị bằng chuột.

**Kiến trúc.** Mỗi bộ cài đăng ký cùng một lệnh mở Tube2Pod từ danh sách ứng dụng. Đề xuất dùng một màn hình thiết lập chung bằng HTML trên loopback, tận dụng Axum và trình duyệt mà Tube2Pod đã dùng. Hộp thoại cài gói ban đầu theo quy ước của từng hệ điều hành. Chế độ chạy dịch vụ và chế độ portable hiện tại vẫn dùng lệnh cũ.

**Công nghệ.** Rust, Axum, HTML/CSS, workflow native hiện có, Inno Setup trên Windows, dpkg-deb trên Ubuntu và pkgbuild trên macOS.

**Thiết kế.** [Đặc tả đã duyệt](../specs/2026-11-25-native-installers-design.md) và [bản mẫu](../specs/assets/native-installer.html).

## Ràng buộc chung

- Windows x64, Ubuntu Desktop x64, macOS Apple Silicon và macOS Intel. Nhãn hệ điều hành hiển thị tên và kiến trúc. Tên tệp tải về giữ định dạng chuẩn.
- Giao diện tiếng Việt theo bản mẫu. Không đưa hai bộ chọn dành cho reviewer vào sản phẩm.
- Cài lần đầu qua Internet, dùng lại công cụ khi mở lần sau. Giữ cấu hình, thư viện, feed và danh tính máy khi cài lại hoặc gỡ. Dùng âm thanh nguồn của video.

## Trường hợp cần kiểm tra

Tải bị ngắt giữa gói FFmpeg sau khi đã nhận yt-dlp (task 1), ổ đầy khi giải nén (task 1), bấm biểu tượng hai lần hoặc cổng đang thuộc một ứng dụng khác (task 2), cấu hình riêng trong đường dẫn có khoảng trắng (task 2), gỡ và cài lại khi thư viện đã có tập phát hành (task 3).

## 1. Tải và kiểm tra công cụ

- [ ] Trong module mới `src/setup/tools.rs`, viết test với nguồn tải cục bộ cho ba trường hợp: lần đầu nhận đủ ba binary chạy được, tải lại sau khi đứt mạng chỉ lấy phần còn thiếu, và lỗi ghi đĩa vẫn bảo toàn công cụ đã cài thành công. Chạy `cargo test --locked setup::tools` để xác nhận các test bắt đúng hành vi còn thiếu.
- [ ] Chọn nguồn và phiên bản cụ thể cho từng kiến trúc từ nguồn được đặc tả chấp nhận. Xác minh đường dẫn tải, cách giải nén, quyền chạy và các thư viện mà binary thực sự cần trên runner sạch. Kiểm tra nội dung tải theo thông tin toàn vẹn của bản phát hành trước khi chạy. Báo riêng nền tảng bị thiếu nguồn phù hợp.
- [ ] Tải vào tệp tạm trong thư mục công cụ của người dùng, chỉ lấy binary mong đợi và kiểm tra chúng bằng cùng lệnh phiên bản đã có trong `src/launch.rs`. Chỉ chuyển công cụ đã kiểm tra vào vị trí sử dụng. Trả tiến độ và nguyên nhân lỗi cho màn hình thiết lập. Khi mọi công cụ đã sẵn sàng, việc mở lại phải thành công cả khi chặn nguồn tải.
- [ ] Chạy lại test trên và kiểm tra phiên bản của cả ba binary trên mỗi hệ điều hành trước khi commit phần tải.

## 2. Màn hình thiết lập và vòng đời ứng dụng

- [ ] Thêm test chế độ `tube2pod --desktop` bên cạnh `tests/portable_launch.rs`. Kiểm tra cấu hình cũ và đường dẫn có khoảng trắng, mở hai lần, cổng owner bị chiếm và chỉ dừng đúng tiến trình đã mở. Chạy `cargo test --locked --test portable_launch` cùng test desktop mới.
- [ ] Thêm nhánh lệnh desktop trong `src/main.rs` và `src/setup/mod.rs`. Dùng mặc định dữ liệu và thứ tự ưu tiên cấu hình của `src/launch.rs`. Mở màn hình thiết lập với quyền của người dùng. Chỉ cho phép trang cục bộ của launcher thực hiện cài, thử lại, mở và dừng. Dùng lại một phiên desktop khi mở lần nữa.
- [ ] Tạo `templates/desktop_setup.html` từ phần cửa sổ của bản mẫu. Nối trạng thái thật cho chuẩn bị, đang tải, mất mạng, hết dung lượng, sẵn sàng và đang chạy. Nút **Mở Tube2Pod** khởi chạy server theo cấu hình hiện hữu và mở trang owner sau khi sẵn sàng. Giữ được thao tác **Dừng Tube2Pod** sau khi đóng tab. Hiển thị lỗi khi cổng thuộc tiến trình khác.
- [ ] So sánh với bản mẫu và thử thao tác bằng chuột trên cài đặt mới. Chạy lại test launcher mới, test portable cũ và `cargo test --locked --all-targets` trước khi commit.

## 3. Tạo gói, cài thử và chuẩn bị bản phát hành

- [ ] Bổ sung bộ cài trong `deploy/installers/` và các bước build trong `.github/workflows/server-packages.yml`. Dùng binary release đã build. Gói Windows tạo mục Start, Ubuntu cài mục desktop, macOS cài Tube2Pod.app trong Applications. Từng mục gọi chế độ desktop. Tệp ứng dụng cài tách khỏi dữ liệu và cấu hình.
- [ ] Bổ sung kiểm tra gói native cạnh `tests/package_scripts.py` và `tests/package_smoke.py`. Kiểm tra kiến trúc, lệnh mở ứng dụng và danh sách tệp được cài. Giữ smoke test của gói portable đang có.
- [ ] Cài từng artifact trên máy hoặc VM có desktop sạch, không cài sẵn công cụ media. Từ biểu tượng, thực hiện tải, thử lại, mở, tạo tập bằng nguồn được phép dùng, phát MP3 qua RSS, dừng và mở lại. Gỡ rồi cài lại, đối chiếu URL feed và audio cũ. Xác minh mở lại khi nguồn tải bị chặn. Ghi riêng kết quả thực tế của từng kiến trúc, kể cả lời nhắc chữ ký của hệ điều hành.
- [ ] Cập nhật hướng dẫn tải và cài trong `README.md`, `README.en.md`, `README.zh-CN.md`. Chuẩn bị bản phát hành với đúng các artifact đã tạo, kết quả kiểm thử và hạn chế thực tế. Kiểm tra liên kết tải của từng gói trước bàn giao.
