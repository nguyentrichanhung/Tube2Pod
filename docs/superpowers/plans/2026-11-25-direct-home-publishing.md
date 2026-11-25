# Direct Home Publishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Đóng gói Tube2Pod cho bốn target native, publish RSS/audio trực tiếp qua HTTPS/NAT tại nhà và áp dụng layout v1 đã duyệt.

**Architecture:** Giữ Axum/Askama/HTMX, SQLite và pipeline nguồn audio hiện có. Caddy chạy riêng trên máy nhà chỉ chuyển tiếp tới public listener; owner UI và LAN pairing giữ đường riêng. Launcher native chạy foreground và dùng chung parser cấu hình Rust để không thực thi file env như mã lệnh.

**Tech Stack:** Rust, Axum, Askama, HTMX, CSS/JS hiện có, PowerShell, POSIX shell, Caddy, yt-dlp/ffmpeg/ffprobe; native GitHub Actions runners để build/test gói.

**Spec:** [Thiết kế đã duyệt](../specs/2026-11-25-direct-home-publishing-design.md), [layout v1](../specs/assets/direct-home-layout-v1.html).

## Global Constraints

- Target: Windows x64, Linux x64, macOS Apple Silicon và macOS Intel.
- Admin `8400`, public `8401` và owner `8402` tiếp tục bind loopback; LAN `8403` không đưa qua Caddy/NAT.
- Chỉ RSS/audio/cover/health hiện có ra Internet; không thêm tài khoản quản trị hoặc tunnel.
- Không Omnivoice, không bắt buộc Tailscale/relay. Giữ relay cũ và ghép LAN cho triển khai đã dùng.
- Gói giải nén chứa binary, cấu hình, launcher và hướng dẫn; công cụ media/Caddy cài riêng. Không thêm GUI installer, tự chạy nền Windows/macOS hay tự mở browser.
- Launcher foreground; Caddy chạy riêng; giữ Linux systemd hiện tại.
- Không đổi feed token/URL/GUID, schema, pipeline, chính sách migration hoặc workflow trạng thái.
- Không sửa checkout dirty `/home/workspace/Tube2Pod`. Thực hiện tại `/tmp/tube2pod-direct-home-20261125`; dữ liệu thử riêng ngoài data thật.
- Không dùng mockup/dữ liệu giả hoặc cross-build để tuyên bố runtime/native/Internet đã đạt.

## Review Focus

1. Config/path có dấu cách, Unicode, dấu `#`, dấu `=` và chuỗi giống shell: giữ nguyên giá trị, không thực thi; Task 1.
2. Config file vắng/sai và env override: báo lỗi rõ, không đổi hành vi bare binary/systemd hoặc ghi đè dữ liệu; Task 1.
3. Direct origin có/không có relay: URL direct không bị phụ thuộc trạng thái relay và không bị báo đã xác minh; Task 2.
4. HTMX thay fragment, theme và màn hẹp: giữ lựa chọn/form, CSRF, bulk-delete/filter; Task 3.
5. Gói đổi thư mục, cổng bận, dừng/restart và HTTP Range: dữ liệu giữ nguyên, tiến trình dừng đúng, public endpoint không lộ admin; Tasks 1, 4.

## File map và thứ tự

1. Runtime/launch: `src/launch.rs`, `src/main.rs`, native launchers/config mẫu và tests trong `src/launch.rs`, `tests/portable_launch.rs`.
2. Direct status/network: `src/web/connection.rs`, `templates/connection.html`, `src/web/mod.rs`, `deploy/Caddyfile.example`, `deploy/direct-https.md`.
3. Layout: CSS và templates/fragments hiện có; chỉ thêm JS sao chép origin nếu cần, giữ `assets/connection.js` và `assets/theme.js`.
4. Native fixtures/packaging: test modules bị ràng buộc Unix, `tests/support/media_tool.rs`, package scripts, CI workflow, hướng dẫn và ghi nhận kết quả trong kế hoạch này.

Thực hiện tuần tự. Task 2 dùng layout riêng theo mẫu; Task 3 áp dụng stylesheet chung và kiểm tra toàn bộ. Task 4 kiểm chứng sản phẩm tích hợp. Không dựng hệ thống hồ sơ mới.

### Task 1: Launcher portable và cấu hình không thực thi shell

**Files:** Create `src/launch.rs`, `deploy/start.sh`, `deploy/start.ps1`, `deploy/portable.env.example`, `tests/portable_launch.rs`; modify `src/main.rs`; preserve `deploy/install-linux-x86_64.sh`/service behavior.

**Interfaces:**
- CLI mới: `tube2pod --config <path>`; bare `tube2pod` giữ chế độ env hiện tại. Flags không hỗ trợ hoặc thiếu path phải lỗi, không khởi động.
- `launch::load_file(path: &Path) -> anyhow::Result<HashMap<String, String>>` đọc UTF-8/BOM và CRLF; mỗi dòng trống/comment hoặc KEY=VALUE. Key theo `[A-Za-z_][A-Za-z0-9_]*`, trim khoảng trắng quanh key/value; bỏ cặp quote đơn/đôi bao ngoài, giữ nội dung bên trong; không expansion, inline comments hay multiline. Duplicate key hoặc quote không đóng báo số dòng, không in secrets.
- `launch::portable_data_dir(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<PathBuf>` dùng native OS: LOCALAPPDATA/Tube2Pod, HOME/Library/Application Support/Tube2Pod, XDG_DATA_HOME/tube2pod hoặc HOME/.local/share/tube2pod. Chỉ áp dụng trong chế độ `--config` khi DATA_DIR không có giá trị hiệu lực.
- Lookup trong main ưu tiên env, sau đó file, sau đó defaults; áp dụng thống nhất Config, BIND_LAN, TUBE2POD_RELAY_ENDPOINT và RUST_LOG. Không mutate global env từ async runtime.
- `launch::check_tools(cfg: &Config) -> anyhow::Result<()>` trong chế độ portable kiểm tra ba lệnh media bằng version flags, timeout hữu hạn và exit status; không kiểm tra/bắt buộc Caddy cho LAN. Bare mode không thêm startup gate.
- `start.sh [config-path]`/`start.ps1 [-Config path]` gọi binary cạnh script, mặc định file `tube2pod.env` cạnh script. Không tự tạo/ghi đè config. Default DATA_DIR do Rust xử lý để không che giá trị trong file. Working directory là thư mục gói, config argument tương đối phải resolve trước đổi cwd. Shell dùng `exec`, PowerShell truyền argument riêng và trả exit code. Không parse file env trong script.

- [ ] Viết tests parser: `load_file_preserves_literal_values` assert `C:\\Media Tools\\ffmpeg.exe`, `$(touch sentinel)`, `a#b=c` không biến đổi/không tạo sentinel; `load_file_rejects_bad_lines` assert duplicate, key sai, thiếu `=`, quote thiếu đóng báo lỗi và số dòng. Kiểm tra BOM, CRLF, quotes, dòng trống/comment.
- [ ] Chạy `cargo test --locked --bin tube2pod launch::`; xác nhận test mới fail trước implementation vì module/parser chưa có.
- [ ] Implement module/interface trên, main chọn CLI/config trước tracing init. In owner URL thực theo port sau khi bind thành công. Không chuyển logic config/pipeline hiện có sang cấu trúc mới.
- [ ] Thêm `tests/portable_launch.rs` chạy binary với temp config và tools fixture: missing file/tool, invalid flag/config/bind, port occupied phải fail có ngữ cảnh; paths có dấu cách giữ đúng. Native media fixture dùng Task 4, test parser/CLI không chờ Task 4.
- [ ] Viết launchers và sample env không chứa DATA_DIR Linux cố định hoặc relay endpoint; hướng dẫn comment cách override DATA_DIR/tools, public origin tùy chọn. Assert syntax bằng `sh -n deploy/start.sh`, PowerShell parser trên Windows.
- [ ] Chạy tests launch/config/main phù hợp và launcher trên Linux với data riêng; bare mode config tests vẫn pass. Không suy diễn Windows/macOS pass khi chưa có native runner.
- [ ] Commit: `feat: add portable foreground launchers and safe config loading`.

### Task 2: Direct HTTPS và trạng thái kết nối đúng nguồn

**Files:** Modify `src/web/connection.rs`, `templates/connection.html`, `src/web/mod.rs`, `deploy/tube2pod.env.example`; create `deploy/Caddyfile.example`, `deploy/direct-https.md`.

**Interfaces:**
- Thêm `public_base_url: String` vào `ConnectionTemplate`, lấy từ `state.cfg.public_base_url` (direct origin), chuỗi rỗng khi thiếu. Giữ `remote_enabled`, `remote_available`, devices/CSRF; relay hiển thị khu riêng gọi là chuyển tiếp.
- Giữ `AppState::effective_public_base_url()` và semantics ưu tiên origin cấu hình cho feed; không thêm cột DB hoặc probe Internet.
- Copy chính xác trạng thái direct: “Đã cấu hình” và “Chưa kiểm tra truy cập từ Internet”. Khi rỗng: “Chưa cấu hình địa chỉ chia sẻ”; không hiển thị URL mẫu.
- Caddy mẫu dùng `podcast.example.com { reverse_proxy 127.0.0.1:8401 }`. Domain này phải được chủ máy thay; không route admin/owner/mobile. Caddy chạy riêng, không do launcher quản lý.

- [ ] Thêm tests `owner_connection_shows_direct_origin_without_relay`, `owner_connection_without_origin_stays_local`, `owner_connection_does_not_claim_internet_verified` trong test module `src/web/mod.rs`; GET `/connection` với Host `localhost:8402`, assert status, URL thật/empty text, trạng thái chưa kiểm tra và controls ghép cặp vẫn có. Dùng state test pool có sẵn.
- [ ] Chạy `cargo test --locked --bin tube2pod owner_connection`; xác nhận các assert mới fail trên template cũ.
- [ ] Implement dữ liệu/template, giữ DOM ids/classes/data-id đang được `assets/connection.js` dùng, các form/action và CSRF. Bố cục direct bên trái/phạm vi public bên phải, ghép cặp/devices bên dưới như mẫu; relay chỉ xuất hiện theo cấu hình cũ.
- [ ] Thêm test trạng thái relay cấu hình độc lập direct và test escaping origin, dùng fixture Runtime hiện có; chạy test public isolation có sẵn và mở rộng danh sách cấm gồm `/connection`, `/settings`, `/api/v1/*` routes mobile thực tế lấy từ router.
- [ ] Viết Caddy mẫu + hướng dẫn DNS/IP/NAT TCP80/443, firewall, local owner, LAN8403, NAT loopback, IP đổi, feed URL migration giữ quyền quyết định chủ máy. Validate bằng `caddy validate --config deploy/Caddyfile.example --adapter caddyfile` khi Caddy có sẵn; không khởi động ACME với domain mẫu.
- [ ] Chạy tests connection/public phù hợp. Ghi rõ validation config khác với truy cập HTTPS từ mạng ngoài.
- [ ] Commit: `feat: support direct home HTTPS publishing status and deployment`.

### Task 3: Áp dụng layout đã duyệt vào UI thật

**Files:** Modify `assets/app.css`, `templates/base.html`, `templates/connection.html`, `templates/studio.html`, `templates/library.html`, `templates/show_detail.html`, `templates/shows.html`, `templates/episode.html`, `templates/settings.html`, `templates/observability.html`, `templates/partials/preview.html`, `templates/partials/episode_rows.html`, `templates/partials/queue.html` chỉ khi cần bố cục. Reuse `assets/theme.js`, `assets/connection.js` và các handler hiện có.

**Interfaces:**
- Giữ routes, names, ids, HTMX targets/attributes và CSRF contracts. Không đổi pipeline, thứ tự trạng thái hay điều kiện action.
- Palette/layout lấy từ tài liệu HTML đã duyệt. Chuẩn hóa CSS hiện có để tránh thêm một lớp override toàn bộ nữa; chỉ bỏ khai báo bị thay trực tiếp, không dọn phần không liên quan.
- Giữ đủ sáu mục nav hiện có, owner-only Kết nối; h1 khoảng 38px desktop/29px mobile, container khoảng 1208px tính cả padding, hai cột nội dung co một cột. Controls tối thiểu 44px; dùng bìa thật/fallback hiện có.
- Không copy mockup JS, fake queue, trạng thái mẫu, URL example, hoặc placeholder QR vào sản phẩm. Giữ Sáng/Tối/Theo hệ thống và persistence của theme.js.

- [ ] Ghi nhận screenshot UI mới hiện tại bằng server data tách biệt trước sửa; đối chiếu bản mẫu đã duyệt. Không khởi động trên DB người dùng.
- [ ] Sửa stylesheet và markup theo từng trang, bắt đầu khung chung + Kết nối, sau đó Tạo tập/Thư viện/Show và những trang còn lại. Feed chia khu URL/nội dung với điều kiện publish; giữ toggle/rotate/publish-ready/Apple link và xác nhận xóa.
- [ ] Chạy existing tests `cargo test --locked --bin tube2pod web::`; sửa expectations chỉ nếu text/layout được duyệt thay đổi, không sửa assertion nghiệp vụ để che regression.
- [ ] Kiểm tra browser với data thật trong DB thử: trang trống và có dữ liệu, lỗi form/CSRF, source preview đủ track/ngôn ngữ, queue fragment, thư viện lọc/phân trang và chọn nhiều, chi tiết show/feed. Không thử xóa dữ liệu ngoài DB thử.
- [ ] Assert browser: đổi theme giữ URL + input đã gõ; reload giữ theme; system đổi theo media; sau HTMX swap số chọn/bulk button khớp; nav và QR/rename/revoke hoạt động như trước. Nếu có thay đổi JS logic, lưu regression test nhỏ cho logic đó; không thêm test chỉ khớp class CSS.
- [ ] Screenshot/kiểm tra 1440 và 390px, keyboard focus/skip link, không overflow; console không có lỗi mới. Chỉ render lại vùng bị lỗi/sửa, không lập thêm vòng kiểm định.
- [ ] Commit: `feat: apply approved home publishing website layout`.

### Task 4: Native verification, gói phát hành và kiểm chứng tích hợp

**Files:** Create `deploy/package.sh`, `deploy/package.ps1`, `.github/workflows/server-packages.yml`, `deploy/portable.md`, `tests/support/media_tool.rs`, `src/test_tools.rs`; modify `src/main.rs` test module registration, `src/e2e.rs`, test sections của `src/services/ffmpeg.rs`, `src/services/ytdlp.rs`, `src/worker/assemble.rs`, `src/worker/fetch.rs`, `src/web/episodes.rs`, `src/web/studio.rs` chỉ cho fixtures native; `README.md`. Giữ Unix permission/symlink tests có cfg tương ứng.

**Interfaces:**
- Native test helper `test_tools::tool(dir: &Path, name: &str, scenario: &str) -> PathBuf`, only cfg(test), biên dịch fixture Rust std-only vào temp cache dùng chung qua OnceLock rồi copy vào dir tên `.exe` trên Windows. Scenario truyền qua sidecar file cạnh executable, không shell/env toàn cục. Fixture: metadata, download-copy, probe, conversion-copy, fail, delayed-write; ghi args để assert selection và paths.
- Thay shell fixtures liên quan bằng helper, bảo toàn metadata/selection/path-escape/failure/cancellation assertions. Tests pure không bị gắn cfg(unix) chỉ vì cùng module có shell tests. Giữ lỗi và timeout của production code.
- `package.sh <target> <binary> <out-dir>` (Unix) và `package.ps1 -Target -Binary -OutDir` (Windows) đóng gói binary, native launcher, portable.env.example, Caddyfile.example và deploy guides. Linux thêm installer/unit/env example hiện có đúng cấu trúc installer. Artifact: `tube2pod-<target>.tar.gz` hoặc `.zip`; không kèm data, keys, env populated, relay binary hay `.git`.
- CI matrix: `ubuntu-24.04`/`x86_64-unknown-linux-gnu`, `windows-2022`/`x86_64-pc-windows-msvc`, `macos-15`/`aarch64-apple-darwin`, `macos-15-intel`/`x86_64-apple-darwin`; test/build locked trên native host, cài tools test từ nguồn chính thức, upload artifacts; không tự deploy server. Labels theo [GitHub runners reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners), đã kiểm tra 25/11/2026.

- [ ] Chạy `cargo test --locked --all-targets` trên Linux làm baseline; ghi lỗi có trước riêng. Tạo fixture/helper native, chuyển tests ở phạm vi trên; thêm test fixture tự kiểm tra args + source file copied + failure + delayed marker không xuất hiện khi bị cancel.
- [ ] Test media thực trên từng native runner: ffmpeg tạo MP3 từ `tests/fixtures/audio_sine.mp3`, ffprobe trả duration>0 và bytes>0, path có dấu cách. Test fixture pipeline không thay cho smoke với yt-dlp thật; ghi riêng hai kết quả.
- [ ] Viết package scripts cùng self-check bằng thư mục/binary giả: archive chứa đúng launcher/config/docs, Unix execute bit giữ; path khoảng trắng hoạt động, input thiếu fail, không đóng gói env thật. Dùng native tar/Compress-Archive, không thêm dependency đóng gói.
- [ ] Viết CI workflow test/build/package, chỉ commit workflow; không tự merge, publish release hay deploy. Việc chạy CI cần branch remote/runner khả dụng; báo thiếu bằng chứng nếu chưa thể chạy.
- [ ] Build Linux release bằng `cargo build --release --locked --bin tube2pod`, tạo gói và giải nén vào path có khoảng trắng. Khởi động launcher, mở owner, giữ DATA_DIR ngoài package, dừng/restart và so sánh show/feed token/audio được giữ. Test missing tools/cổng bận thoát rõ.
- [ ] Hướng dẫn cài dependencies native, KEY=VALUE grammar/precedence, hai terminal cho server và Caddy, dữ liệu bền vững, Ctrl+C, upgrade backup, Linux systemd. Không hướng dẫn tắt bảo vệ toàn OS.
- [ ] Caddy test local trên port cao với config thử chỉ proxy public: fetch feed/audio/cover, HEAD/Range phải hoạt động; admin/owner/mobile phải 404. Không đổi mẫu production80/443 để phù hợp môi trường test.
- [ ] Với tên miền/router thật do chủ máy cấu hình và nguồn có quyền sử dụng: tạo một tập qua yt-dlp thật, publish, từ mạng ngoài mở HTTPS feed bằng app podcast, phát và seek audio. Thiếu quyền truy cập/router/domain/nguồn thì ghi “chưa kiểm chứng”, không dùng fixture hay LAN làm thay thế.
- [ ] Chạy `cargo fmt --check`, `cargo test --locked --all-targets`, `git diff --check` sau các sửa liên quan; ghi riêng artifact chuẩn bị/build/run/pass cho bốn target. Không chạy lặp khi không có thay đổi hoặc lỗi mới.
- [ ] Review độc lập toàn nhánh bởi advisor đúng role trước bàn giao; sửa chỉ finding trong scope rồi retest phần ảnh hưởng. Kết quả thực còn thiếu không được xem là hoàn thành toàn bộ.
- [ ] Commit: `build: package and verify native home server releases`.

## Kết quả xác minh (điền khi thực hiện)

| Hạng mục | Kết quả 25/11/2026 |
| --- | --- |
| Thiết kế và layout | Người dùng đã duyệt |
| Linux x64 artifact / runtime | 1 artifact build/run/pass trên Ubuntu22.04.5 x64; source thật67s → MP3/publish/restart giữ show/token/audio |
| Windows x64 artifact / runtime | Workflow đã chuẩn bị; 0 build/run/pass native, chưa chạy CI |
| macOS arm64 artifact / runtime | Workflow đã chuẩn bị; 0 build/run/pass native, chưa chạy CI |
| macOS x64 artifact / runtime | Workflow đã chuẩn bị; 0 build/run/pass native, chưa chạy CI |
| UI sản phẩm với backend | Task3 browser1440/390, theme, preview fixture, feed actions và cover đạt; Task4 source preview/pipeline thật qua HTTP đạt |
| HTTPS local proxy / public isolation | Caddy local CA verified: RSS/cover/audio200, HEAD200, Range206; 8admin/mobile routes404 |
| Domain thật, NAT, app podcast từ ngoài nhà | Chưa kiểm chứng: chưa có hostname DuckDNS đăng ký, router forwarding và thiết bị ngoài mạng |

## Tự review kế hoạch

Đã đối chiếu spec: packaging/lifecycle/data nằm Tasks1/4; HTTPS/public/relay Tasks2/4; toàn bộ layout và hành vi cũ Task3; native/media/Internet bằng chứng Task4. Năm failure modes có checks cụ thể ở tasks sở hữu. Không thay sản phẩm bằng mockup, không thêm framework hoặc hệ thống quyền mới.

Thiết kế đã duyệt không cần hỏi lại. Theo writing-plans, chờ người dùng review kế hoạch và chọn Native hoặc Subagent-driven trước triển khai. Đề xuất Native: bốn task dùng chung startup/templates/test fixtures nên làm tuần tự rồi một review độc lập toàn nhánh giảm chi phí điều phối.
