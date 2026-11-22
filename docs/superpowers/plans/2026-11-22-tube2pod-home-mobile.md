# Tube2Pod Home Server and Mobile Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Kết quả** — Cài Tube2Pod Server trên Linux x86-64, tạo podcast từ YouTube trên website, ghép app Android/iOS để nghe và phát hành, rồi bật truy cập Internet khi cần.

**Kiến trúc** — Máy ở nhà chạy Axum, Askama, HTMX, SQLite và worker MP3 hiện có. App Flutter ưu tiên kết nối LAN đã ghép cặp, đồng thời dùng dịch vụ chuyển tiếp Tube2Pod nếu chủ máy bật và đường LAN không khả dụng. Qua relay, app quản trị dùng TLS tới máy ở nhà, còn RSS, MP3 và ảnh bìa được phục vụ qua HTTPS công khai tiêu chuẩn.

**Công cụ** — Rust theo mức tối thiểu `1.85` trong `Cargo.toml`, Tokio, Axum, SQLite/SQLx, Flutter, API nền tảng Android/iOS, Bonjour/mDNS, TLS và các công cụ `yt-dlp`, `ffmpeg`, `ffprobe` hiện có.

**Spec đã duyệt** — [Kiến trúc và nghiệm thu](../specs/2026-11-22-local-first-mobile-design.md) · [Thiết kế frontend v3](../specs/2026-11-22-tube2pod-frontend-design.md). Bản Visual Companion tương ứng trong workspace thiết kế là `.superpowers/brainstorm/1398795-1795320190/content/tube2pod-home-mobile-v3.html`.

## Phạm vi chung

- Bắt đầu từ `bccac7f` và hai spec đã duyệt. Dùng worktree triển khai riêng và dữ liệu thử riêng.
- Website giữ luồng dán link YouTube, chọn show, chọn audio, chuyển toàn bộ bản audio sang MP3, thử lại, hủy và xóa. Giữ mặc định ngôn ngữ đã duyệt, chống trùng theo khóa video và luật tự phát hành. App đọc nội dung Tube2Pod, quản lý show và phát hành tập đang **Sẵn sàng**.
- SQLite, MP3, bìa, bản nhận diện máy và hàng đợi ở máy nhà. Giữ migration `0001`–`0006`, token feed, ID show, UUID/GUID tập và tệp còn hợp lệ. Dùng bản sao khi kiểm tra nâng cấp. Chủ máy quyết định khi nào đổi URL công khai.
- Chủ máy cấp QR ngắn hạn, dùng một lần, tại website loopback và thu hồi từng thiết bị. Quyền cấp riêng áp dụng ở LAN và Internet. Các thay đổi bảo mật ngoài kiến trúc đã duyệt cần xin ý kiến người dùng.
- Bản đầu dùng Linux x86-64 và app Flutter Android/iOS. Dùng theme **Sáng**, **Tối**, **Theo hệ thống**, mặc định theo hệ thống và lưu lựa chọn riêng tại từng trình duyệt hoặc thiết bị. Đối chiếu vị trí điều hướng, màu và thao tác với frontend v3.
- Chủ máy tại nhà có thể cài và nghe qua LAN trước khi bật relay. Relay tùy chọn do Tube2Pod vận hành, cấp địa chỉ công khai sẵn. `PUBLIC_BASE_URL` HTTPS đang cấu hình tiếp tục được ưu tiên.

## Tổ chức công việc

| Vùng | Tệp và trách nhiệm |
| --- | --- |
| Khởi động và truy cập ở nhà | `src/config.rs`, `src/main.rs`, `src/state.rs`, `src/web/csrf.rs` và `src/web/mod.rs` giữ cấu hình, origin, router và listener tách biệt. |
| Thiết bị và dữ liệu app | `src/device/identity.rs`, `src/device/lan.rs`, `src/db/devices.rs` và `migrations/0007_devices.sql` quản lý định danh, LAN và ghép cặp. `src/web/connection.rs` phục vụ chủ máy. `src/mobile_api.rs` chuyển yêu cầu app sang nghiệp vụ ở nhà. |
| Dữ liệu podcast | `src/services/shows.rs` dùng chung nghiệp vụ show, `src/db/shows.rs` và `src/db/episodes.rs` vẫn ghi SQLite. `src/web/public.rs` và `src/services/feed.rs` giữ RSS, enclosure và bìa. |
| Web và app | `templates/` và `assets/` phục vụ frontend web. `apps/mobile/lib/app/` chứa shell và theme. `apps/mobile/lib/features/` chứa `connection`, `listen`, `shows` và `admin`. Adapter thiết bị nằm trong `apps/mobile/lib/platform/` và các thư mục Android/iOS. |
| Chuyển tiếp và phát hành | `src/relay/` chứa hợp đồng kết nối đi ra và kết nối đang hoạt động, `src/bin/tube2pod-relay.rs` là dịch vụ do Tube2Pod vận hành. `deploy/` và `README.md` chứa gói cài và cách xác minh phát hành. |

Mục 1 thử sớm tính khả thi trên Android/iOS và relay. Mục 2 có thể làm độc lập với thử nghiệm đó. Sau khi có hợp đồng ở Mục 3, Mục 4 và phần giao diện của Mục 5 có thể làm song song. Mục 6 dùng cùng định danh và API đã kiểm tra ở nhà. Mục 7 tổng hợp nghiệm thu và chuẩn bị phát hành.

## Trọng tâm review

| Trường hợp | Kết quả cần kiểm tra |
| --- | --- |
| Trình phát native dùng client khác client API | Mục 1 xác nhận phát và tua qua đúng khóa máy đã ghép trên Android và iOS. |
| Cùng một mã được gửi đồng thời và hai điện thoại đã cấp quyền | Mục 3 xác nhận mã chỉ cấp một lần, thu hồi A chỉ chặn A. |
| Máy nhà đổi IP, mDNS tạm mất hoặc relay đưa nhầm máy Tube2Pod | Mục 5 kiểm tra lại đường LAN và QR. Mục 6 xác nhận pin sai bị từ chối trước khi gửi thông tin đăng nhập. |
| App mất phản hồi ngay sau khi máy nhà đã ghi phát hành | Mục 3 và 5 kiểm tra đọc lại cùng một UUID và giữ ngày phát hành. |
| Nâng cấp host đã có feed hoặc ngắt riêng đường quản trị | Mục 2 và 6 kiểm tra HTTPS/link cũ, GUID, `HEAD`, `Range` và đường nghe vẫn hoạt động khi đường quản trị tạm mất. |

---

## 1. Kiểm tra đường TLS và audio trên thiết bị thật

**Tệp** — Tạo khung `apps/mobile/` cho Android/iOS, `apps/mobile/integration_test/transport_test.dart`, `apps/mobile/lib/platform/home_transport.dart` và adapter native tương ứng. Tạo `tests/native_transport_fixture.rs` cho phép thử TLS riêng, dùng lại `tests/fixtures/audio_sine.mp3`.

**Giao diện** — Flutter gọi `HomeTransport.request(HomeEndpoint, HomeRequest)` và `HomePlayer.open(HomeMediaSource)`. `HomeEndpoint` gồm `uri`, `serverId`, `serverKey` (khóa công khai SPKI). `HomeMediaSource` phân biệt media của máy đã ghép và enclosure HTTPS. Giữ cùng cách kiểm tra pin cho API và các lượt đọc media của máy đã ghép.

- [ ] Viết `transport_test.dart` cho đủ bốn kết quả. Đúng khóa trả nội dung API và phát được MP3 có tua đến nửa tập. Sai khóa trả `identityMismatch` trước khi fixture nhận thông tin đăng nhập. Enclosure HTTPS chuẩn phát và tua được qua `Range`. Kết nối admin thử qua relay dùng cùng khóa máy ở nhà.
- [ ] Chạy `cd apps/mobile && flutter test integration_test/transport_test.dart -d <device-id>` trên Android và iOS với fixture tạm, xác nhận các ca đang thiếu adapter chưa đạt.
- [ ] Tạo fixture TLS và relay mẫu để kiểm tra một luồng TLS nguyên vẹn khi máy nhà chỉ mở kết nối đi ra. Chạy `cargo test --locked --test native_transport_fixture -- --ignored --nocapture`. Dùng API xác minh TLS native và nguồn dữ liệu media dùng cùng client đã xác minh. Trên iOS, kiểm tra đường cung cấp range cho AVPlayer nếu player thông thường không dùng verifier của API. [TLS LAN trên Apple](https://developer.apple.com/documentation/network/creating-an-identity-for-local-network-tls) · [AVFoundation resource loader](https://developer.apple.com/documentation/avfoundation/avassetresourceloadingdatarequest) · [Flutter platform channels](https://docs.flutter.dev/platform-integration/platform-channels)
- [ ] Chạy lại trên cả hai thiết bị. Ghi phiên bản hệ điều hành, cấu hình LAN, kết quả API/phát/tua và kết quả TLS qua relay. Mang adapter đạt thử nghiệm sang Mục 5. Khi thiếu thiết bị, ghi thiếu bằng chứng cho nền tảng đó và tiếp tục phần server/web độc lập.
- [ ] Commit khung Flutter và ca kiểm tra transport có thể dùng lại.

## 2. Khởi động tại nhà và bảo toàn feed khi nâng cấp

**Tệp** — Sửa `src/config.rs`, `src/main.rs`, `src/state.rs`, `src/web/mod.rs`, `src/web/csrf.rs`, `src/web/public.rs`, `src/web/shows.rs`, `src/services/feed.rs`, `src/e2e.rs` và `deploy/tube2pod.env.example`.

**Giao diện** — `Config::public_base_url: Option<Url>`. `AppState::effective_public_base_url(&self) -> AppResult<Option<Url>>` ưu tiên origin được cấu hình rồi đến origin relay khi đường public của relay khả dụng. `owner_router(state: AppState, origin: Url) -> Router` mặc định tại `http://localhost:8402` trên listener loopback riêng. `admin_router` và `public_router` giữ cổng `8400` và `8401` theo mặc định cũ.

- [ ] Thêm `boots_without_public_origin`, `owner_origin_uses_exact_localhost_port`, `legacy_admin_keeps_https_origin_policy`, `published_audio_only_on_public_listener` và `upgrade_retains_existing_feed_items`. Kiểm tra khởi động với origin rỗng, browser POST tại `http://localhost:8402` với CSRF đúng, từ chối origin/host sai và vẫn hỗ trợ HTTPS reverse proxy cũ. Fixture SQLite từ bản `bccac7f` có feed đã bật, tập đã phát hành và tập mới **Sẵn sàng**. RSS, `HEAD`, `Range` và bìa tập phát hành đạt, còn tập chưa phát hành và router quản trị vắng trên listener công khai.
- [ ] Chạy `cargo test --locked config::tests`, `cargo test --locked web::` và `cargo test --locked e2e::`. Xác nhận các ca mới lỗi đúng ở origin bắt buộc, phạm vi router hoặc điều kiện media chưa phát hành.
- [ ] Làm public origin tùy chọn trong `Config::from_lookup`. Khi không có origin, web hiển thị feed chưa có địa chỉ chia sẻ. Khi có origin, dùng URL tuyệt đối ở feed, enclosure và bìa. Giữ membership lịch sử dựa trên `published_at` và tệp hoàn chỉnh. Đồng bộ điều kiện audio công khai với membership đó. Giữ cache/ETag đúng với origin hiệu lực.
- [ ] Thêm listener owner loopback và xác minh host/origin đúng `BIND_OWNER`, giữ cookie và token CSRF. Chỉ router owner phục vụ thao tác cấp quyền sau này. Kiểm thử cookie và lượt ghi bằng Chrome trên localhost.
- [ ] Chạy lại ba nhóm test. Trên bản sao dữ liệu, so token, ID, GUID, URL RSS, thời lượng và byte range trước/sau. Tập hợp lệ cũ đọc được và luồng YouTube vẫn qua `src/e2e.rs`.
- [ ] Commit tương thích server ở nhà và feed.

## 3. Ghép cặp, quyền thiết bị và hợp đồng quản trị

**Tệp** — Tạo `migrations/0007_devices.sql`, `src/db/devices.rs`, `src/device/{mod,identity,lan}.rs`, `src/mobile_api.rs`, `src/web/connection.rs` và `src/services/shows.rs`. Sửa `src/db/mod.rs`, `src/main.rs`, `src/web/mod.rs`, `src/web/shows.rs` và `src/web/episodes.rs`.

**Giao diện** — `PairEnvelopeV1 { version, server_id, lan_uri, server_key, ticket, expires_at }`. `server_key` là SPKI được mã hóa base64url. LAN quảng bá `_tube2pod._tcp.local` với `server_id`. SQLite lưu ticket và quyền thiết bị riêng. `devices::redeem(pool: &SqlitePool, ticket: &str, label: &str, now: i64) -> AppResult<DeviceGrant>` tiêu thụ ticket và tạo grant cùng giao dịch. `devices::revoke(pool: &SqlitePool, device_id: &str) -> AppResult<()>` chỉ thu hồi ID đó.

**Điểm gọi từ website tại máy** — `POST /connection/pair-ticket` tạo mã có hạn 300 giây như bản xem thử và hủy mã chưa dùng trước đó. `POST /connection/devices/{id}/rename` đổi nhãn thiết bị. `POST /connection/devices/{id}/revoke` thu hồi một thiết bị. Tất cả dùng CSRF qua router owner của Mục 2.

**Hợp đồng app** — JSON snake_case, `Authorization: Bearer <credential>` cho lượt yêu cầu đã ghép. Dùng các giá trị trạng thái `ready` và `published` trong dữ liệu app, dịch nhãn ở UI. `show_id` dùng ID SQLite, `episode_id` dùng UUID hiện có.

| Điểm gọi | Kết quả và trường dùng chung |
| --- | --- |
| `POST /v1/pair` | LAN qua TLS, nhận `ticket`, `label`. Trả `device_id`, `credential` và `server_id`. Mã lỗi `pair_used`, `pair_expired`, `pair_cancelled` để app hướng dẫn tạo QR mới. |
| `GET /v1/connection` | Trả `server_id`, `admin_uri` và `public_origin` khi khả dụng. Hai địa chỉ có thể rỗng trong cài đặt LAN. |
| `GET /v1/shows`, `GET /v1/shows/{id}`, `POST /v1/shows`, `PATCH /v1/shows/{id}` | `id`, `title`, `description`, `author`, `category`, `language`, `feed_enabled`, `public_feed_url` tùy chọn, `cover_path` tương đối và `ready_count`. PATCH chỉ ghi trường được gửi. |
| `POST /v1/shows/{id}/cover`, `PUT /v1/shows/{id}/feed` | Ảnh multipart field `cover`, hoặc JSON có `enabled`. Trả show đã cập nhật. |
| `GET /v1/shows/{id}/episodes?status=ready|published`, `GET /v1/episodes/{uuid}`, `POST /v1/episodes/{uuid}/publish` | `id`, `show_id`, `title`, `description`, `language`, `duration_seconds`, `status`, `published_at`, `media_path` khi khả dụng. Phát hành ghi đúng tập **Sẵn sàng**. App đọc lại GET sau khi mất phản hồi hoặc khi tập đã đổi trạng thái. |
| `GET/HEAD /v1/episodes/{uuid}/audio`, `GET/HEAD /v1/shows/{id}/cover` | Media riêng của máy đã ghép. Audio app nghe chỉ cho tập đã phát hành. Giữ `Range` cho player. |

- [ ] Thêm `pair_redeems_once_concurrently`, `pair_rejects_expired_or_cancelled_ticket`, `revoking_one_device_preserves_other_device`, `pair_route_is_lan_only`, `publish_readback_keeps_uuid_and_timestamp`, `mobile_and_web_share_show_validation`. Kiểm tra hai lần đổi mã đồng thời chỉ nhận một credential, mã cũ không dùng được sau khi tạo lại, mỗi thiết bị có quyền riêng, và đường công khai vắng các API trên.
- [ ] Chạy `cargo test --locked db::devices`, `cargo test --locked mobile_api` và `cargo test --locked web::shows`. Chạy test mới cùng khi khai báo tối thiểu module/router để xác nhận chúng chưa đạt.
- [ ] Dùng identity TLS bền vững ở thư mục dữ liệu. Tạo ticket ngắn hạn, cấp từ owner, không đặt mã trong URL mạng. `POST /v1/pair` đi tới adapter LAN và dùng transaction SQLite. Mọi yêu cầu app sau đó tra quyền thiết bị hiện tại. Web owner xử lý tạo mã, thu hồi và đổi tên thiết bị, app chỉ có quyền đã cấp.
- [ ] Chuyển nghiệp vụ create/update show và lưu bìa từ `src/web/shows.rs` sang `src/services/shows.rs` dùng chung. Giữ validation, mặc định chủ sở hữu, giới hạn file và cover version. Dùng `db::shows::set_feed_enabled` và `db::episodes::publish_one` cho các thao tác tương ứng. Đọc lại show/tập sau khi ghi. Giữ quản lý YouTube, xóa và đổi token trong web.
- [ ] Chạy lại các nhóm test và `cargo test --locked`. Xác nhận bản web có dữ liệu tạo từ API, app thấy tập website tạo, và trạng thái feed độc lập với phát hành.
- [ ] Commit hợp đồng ghép cặp và API app.

## 4. Website v3 và giao diện theo hệ thống

**Tệp** — Tạo `templates/connection.html` và `assets/theme.js`. Sửa `templates/base.html`, `studio.html`, `library.html`, `episode.html`, `shows.html`, `show_detail.html`, `observability.html`, `settings.html`, các partial cần đổi bố cục và `assets/app.css`.

**Giao diện** — Giữ các tuyến HTML và HTMX hiện có. Thêm `GET /connection` và thao tác POST của owner ở Mục 3. Bộ chọn giao diện giá trị `system|light|dark`, lưu trình duyệt, theo `prefers-color-scheme` khi chọn hệ thống.

- [ ] Gắn vào ca kiểm tra browser `owner_web_v3_preserves_create_and_settings` các hành động Kết nối, tạo QR, đổi tên và thu hồi riêng từng thiết bị, dán URL, xem trước, chọn bản đi kèm, xác nhận ngôn ngữ, tạo tập, xem Thư viện và lưu Cài đặt. Kiểm tra chủ máy trực tiếp và kiểm tra đường HTTPS cũ với router tương ứng. Thêm kiểm tra theme mặc định, đổi hệ thống, hai ghi đè và giá trị sau tải lại.
- [ ] Chạy qua Chrome trên bản thử lưu trữ tạm để thấy phần Kết nối và theme còn thiếu. Đối chiếu layout v3 ở 1440, 1024, 768 và 390 px.
- [ ] Dùng shell điều hướng ngang và hai hàng trên màn hẹp. Dùng QR nổi bật trên hàng riêng, sau đó thiết bị và truy cập ngoài nhà. Đặt xem trước video ở trên hàng đợi, các show dạng lưới bìa và cài đặt dạng nhóm. Giữ trường link, lựa chọn show/audio/ngôn ngữ, xác nhận, bộ lọc và phân trang. Hỗ trợ thao tác bàn phím và giữ dữ liệu form khi đổi theme.
- [ ] Chạy lại browser trên hai bảng màu và `cargo test --locked web::`. Kiểm tra luồng tạo tập bằng fixture nhiều bản audio và nguồn thử được phép, sau đó xem trực tiếp toàn bộ thời lượng MP3.
- [ ] Commit website v3.

## 5. App Flutter v3, nghe và quản trị trong LAN

**Tệp** — Sửa `apps/mobile/lib/main.dart`, `apps/mobile/lib/app/{app,theme}.dart`, adapter của Mục 1, tạo `apps/mobile/lib/features/connection/connection_controller.dart`, `listen/listen_screen.dart`, `shows/shows_screen.dart`, `admin/admin_screen.dart` và `apps/mobile/integration_test/home_lan_test.dart`. Cập nhật `apps/mobile/ios/Runner/Info.plist` và `apps/mobile/android/app/src/main/AndroidManifest.xml` theo quyền cần dùng.

**Giao diện** — `ConnectionController.pair(PairEnvelopeV1)`, `reconnect()` và `ConnectionStatus` với LAN, từ xa, mất kết nối hoặc thiết bị bị thu hồi. `HomeApi` đọc và ghi theo Mục 3. Parser RSS đọc feed được chia sẻ và enclosure HTTPS cùng thư viện Nghe. `ThemeMode.system` là mặc định, hai lựa chọn thủ công lưu trên thiết bị.

- [ ] Thêm `scan_pair_and_play_on_lan`, `reconnects_after_lan_address_changes`, `pair_works_with_mdns_unavailable`, `retains_draft_on_failed_save`, `reads_episode_after_lost_publish_response` và `theme_tracks_os_until_overridden`. Ca LAN kiểm tra quét mã và trạng thái quyền, phát MP3/nghe public feed, tua, thu hồi, và kết nối lại sau khi IP máy ở nhà đổi. Ca quản trị kiểm tra tạo/sửa show, bìa, bật/tắt feed và phát hành đúng tập. Thử quyền camera hoặc LAN bị từ chối rồi cấp lại.
- [ ] Chạy `cd apps/mobile && flutter test`, sau đó `flutter test integration_test/home_lan_test.dart -d <device-id>` để xác nhận phần chưa triển khai. Ghi target SDK và kiểm tra yêu cầu quyền LAN Android tương ứng. [Quyền mạng nội bộ Android](https://developer.android.com/privacy-and-security/local-network-permission) · [Bonjour và quyền mạng nội bộ của iOS](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)
- [ ] Quét mã và kiểm tra pin trước khi gửi ticket. Lưu credential theo máy ở iOS Keychain và kho bảo mật hệ điều hành trên Android. Dùng mDNS để tìm lại máy, giữ địa chỉ QR làm đường dự phòng khi LAN chặn khám phá. Kết nối Internet thông thường của feed dùng tin cậy HTTPS hệ thống. Dùng adapter đã qua Mục 1 cho audio từ máy ghép.
- [ ] Xây shell v3 với **Nghe**, **Show**, **Quản trị** ở đầu, nút **Kết nối**, chọn giao diện, bìa vuông, thư viện lưới và trình phát thu gọn. Màn Quản trị dùng API đã ghép, hiển thị tập sẵn sàng và URL RSS tùy khả dụng. Đổi theme giữ người dùng ở màn hiện tại cùng lựa chọn đang nhập.
- [ ] Chạy `flutter analyze`, `flutter test` và ca LAN trên Android/iOS thật. Ghi kết quả riêng từng thiết bị và điều kiện Wi-Fi. Đối chiếu hai theme với bản xem thử.
- [ ] Commit app nghe và quản trị LAN.

## 6. Relay tùy chọn cho RSS công khai và quản trị đầu-cuối

**Tệp** — Tạo `src/relay/{mod,registration,client,ingress}.rs`, `src/bin/tube2pod-relay.rs` và cấu hình vận hành relay trong `deploy/`. Sửa `src/main.rs`, `src/state.rs`, `src/web/connection.rs`, `templates/connection.html`, `src/mobile_api.rs`, adapter app và ca thử remote. Cập nhật dữ liệu lưu địa chỉ cấp nếu cần qua migration bổ sung, giữ nguyên các migration cũ.

**Giao diện** — `RelayRegistration { id, public_origin, admin_uri, credential }`. `POST /connection/remote` nhận `enabled` trên router owner. Máy nhà đăng ký khi chủ máy bật **Truy cập ngoài nhà** và giữ cùng đăng ký qua restart, ngắt và bật lại. Client relay mở hai kênh đi ra có xác thực cho `Public` và `Admin`. `open_route(id, kind) -> Result<AsyncStream>` mở stream tới đúng máy đăng ký và đúng loại kênh. Public route dùng router RSS hiện có. Admin route chuyển nguyên byte TLS tới router app trên máy nhà, nơi có khóa khớp QR.

- [ ] Thêm `registration_survives_reconnect`, `admin_tunnel_tls_ends_at_home`, `misrouted_home_is_rejected_before_credentials`, `public_feed_survives_admin_tunnel_loss`, `relay_toggle_preserves_legacy_origin_and_publish_state` và kiểm tra `HEAD`, `Range`, bìa, RSS trên public HTTPS.
- [ ] Chạy `cargo test --locked relay::` và ca integration remote trên app. Dùng môi trường thử nơi máy nhà chỉ có kết nối đi ra. Kỳ vọng ca địa chỉ ổn định, TLS đầu-cuối và tách hai đường chưa đạt trước khi có relay.
- [ ] Tạo định danh đăng ký và địa chỉ HTTPS ổn định trên tên miền do Tube2Pod vận hành. Hướng kỹ thuật đề xuất là tunnel qua TLS với multiplex byte stream sẵn có như Yamux, và ingress admin chuyển tiếp TLS theo định danh host. Dùng kết quả Mục 1 để chốt cách vận chuyển cụ thể khi triển khai. Khóa máy ở nhà giữ nguyên giữa LAN và relay. [Yamux](https://github.com/libp2p/rust-yamux) · [TLS 1.3](https://www.rfc-editor.org/rfc/rfc8446)
- [ ] Đường public chuyển streaming, `HEAD`, HTTP byte range và header feed dựa trên public router. Đường admin giữ TLS đến máy nhà. Giới hạn chuyển tiếp vào đúng hai router và chỉ cấp ghép cặp qua LAN. Khi tắt relay, vẫn dùng LAN và origin cấu hình riêng nếu có.
- [ ] App cập nhật `admin_uri` từ `GET /v1/connection` của máy đã xác minh, lưu để dùng khi ra ngoài. Khi LAN không tới được, dùng địa chỉ chuyển tiếp đã nhận. Nếu app chưa từng nhận địa chỉ trước khi ra ngoài, màn Kết nối hướng dẫn kết nối lại tại nhà. Khi khóa không khớp, dừng và báo lỗi định danh.
- [ ] Chạy lại test, thử stream trên hai điện thoại dùng dữ liệu di động và thử luồng đọc RSS bằng trình khách tiêu chuẩn. Ngắt riêng kênh admin và xác nhận public còn nghe. Thu hồi một máy tại web chủ và kiểm tra cả LAN lẫn relay. Xác nhận bật lại giữ địa chỉ đã cấp và URL hiện có.
- [ ] Commit relay và tích hợp truy cập từ xa.

## 7. Cài đặt, nghiệm thu và chuẩn bị bản store

**Tệp** — Sửa `deploy/tube2pod.service`, `deploy/tube2pod.env.example`, thêm `deploy/install-linux-x86_64.sh`, cập nhật `README.md` và ca xuyên suốt trong `src/e2e.rs` cùng `apps/mobile/integration_test/`.

**Đầu ra** — Gói Tube2Pod cho Linux x86-64, bản thử Android/iOS, máy chủ demo độc lập dùng nội dung có quyền phân phối và ghi chú cung cấp quyền truy cập cho đội review. Địa chỉ dùng public RSS do Tube2Pod cấp hoặc dùng origin đã cấu hình.

- [ ] Trên máy Linux x86-64 sạch, thử cài gói bằng binary release và unit systemd hiện có. Kiểm tra `yt-dlp --version`, `ffmpeg -version`, `ffprobe -version` với tài khoản dịch vụ. Mở website owner bằng link localhost, ghép Android/iOS trong Wi-Fi và nghe khi chưa cấu hình tên miền riêng.
- [ ] Chạy `cargo fmt --check`, `cargo test --locked`, `cargo build --release --locked`, `flutter analyze`, `flutter test` và các ca trên Android/iOS thật. Tạo bản ứng viên `flutter build appbundle` và `flutter build ipa` trên môi trường có SDK và quyền ký tương ứng.
- [ ] Trên bản sao cài đặt cũ, thực hiện chu trình URL YouTube được phép sử dụng → chọn bản audio → full MP3 → app phát hành → đọc RSS từ Internet. Kiểm tra thời lượng với `ffprobe`, GUID và enclosure với XML, `HEAD`, `206`, bìa, thiết bị bị thu hồi và bước đổi IP. Khi chủ máy duyệt đổi URL, kiểm tra redirect `301` từ URL cũ nếu còn quyền quản lý endpoint đó. [Yêu cầu RSS Apple Podcasts](https://podcasters.apple.com/support/823-podcast-requirements) · [Đổi URL feed trên Apple Podcasts](https://podcasters.apple.com/support/837-change-the-rss-feed-url)
- [ ] Chuẩn bị dữ liệu demo, hướng dẫn cấp QR mới và các bước nghe, sửa show, phát hành. Xác nhận người duyệt thực hiện được luồng đầy đủ với máy demo, gồm khả năng dùng lại thông tin truy cập trong thời gian review. Nếu reviewer không thể ghép với máy demo qua LAN, đưa cách demo khác cho người dùng duyệt trước khi áp dụng. Điểm này thuộc phần nộp store, phần phát triển và nghiệm thu nội bộ tiếp tục theo phạm vi đã chốt. [Apple App Review](https://developer.apple.com/app-store/review/guidelines/) · [Google Play App Access](https://support.google.com/googleplay/android-developer/answer/15748846?hl=en)
- [ ] Ghi riêng số máy chủ, thiết bị, URL và tập đã thử cùng kết quả đạt, lỗi hoặc chưa xác minh. Review độc lập các thay đổi về relay, quyền, truy cập dữ liệu và nâng cấp. Commit gói cài, hướng dẫn và các kiểm tra đã chạy.

Các bước triển khai lên máy đang chạy và gửi bản ứng viên ra store theo quyết định phát hành của người dùng.
