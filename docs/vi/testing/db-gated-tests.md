# Test cần DB: bỏ qua không còn là "đạt"

Bản tham chiếu. Bản chuẩn (canonical): `docs/en/testing/db-gated-tests.md`.

Hầu hết integration suite trong workspace này cần Postgres. Đến 2026-08-30, tất cả đều
xử lý việc thiếu Postgres theo cùng một cách: `egret_test_db::url()` trả `None`, macro
`skip_without_db!` biến nó thành `return` sớm, và libtest in ra `ok`.

Một suite **không assert gì** trông **y hệt** một suite đã chạy thật — chỉ khác thời
lượng. Đo trên `cargo test -p egret-dataapi`, cùng máy, cùng commit:

| lần chạy | binary | báo cáo | thời gian |
| --- | --- | --- | --- |
| có `DATABASE_URL` | 23 | `373 passed; 0 failed` | 282.69 s |
| không `DATABASE_URL` | 23 | `373 passed; 0 failed` | 11.31 s |

Đầu ra giống hệt nhau. 315 trong 373 test đó không chạy một assertion nào ở lần thứ hai
(chỉ 9 test của `serving_auth.rs` và 49 của `template_render.rs` là không cần DB theo
thiết kế). Một cuộc audit Data API đã phải ghi chú gần như mọi phát hiện bằng câu "suite
báo `ok` nhưng có thể chưa assert gì".

## Quy tắc hiện tại

`egret_test_db::url()` **panic** khi không lấy được test database. Vẫn bỏ qua được, nhưng
phải yêu cầu rõ ràng:

```bash
EGRET_ALLOW_DB_SKIP=1 cargo test --workspace
```

Việc bỏ qua bây giờ nằm trong chính dòng lệnh tạo ra màu xanh, thay vì chỉ suy ra được từ
đồng hồ. Cùng một panic đó cũng bao trùm trường hợp `DATABASE_URL` có nhưng **không kết
nối được** — trường hợp này trước đây cũng im lặng bỏ qua, và nó chính là hình dạng của
một lần chạy CI khi service database không khởi động được.

### Opt-out KHÔNG được đến từ `.env`

Toàn bộ lời khẳng định "việc bỏ qua nằm trong dòng lệnh" chỉ đáng giá bằng đúng bảo đảm
rằng biến đó không thể đến từ chỗ không ai nhìn. Một `.env` phá vỡ nó bằng một dòng không
được commit: `justfile` có `set dotenv-load := true`, nên `.env` vào môi trường của **mọi**
recipe; và `dotenvy::dotenv()` mà `#[sqlx::test]` gọi cũng nạp cả file vào môi trường tiến
trình. Cả hai đường đều khiến `just check` báo xanh toàn phần trong khi không assert gì, và
không có gì hiện trên terminal — thông báo skip bị libtest nuốt **chính vì** test đạt.

Vì vậy `egret_test_db` đọc **file**, không đọc môi trường, và **từ chối** một opt-out mà
một `.env` ở thư mục hiện tại hoặc cấp trên có thể đã cung cấp. Đây là biện pháp mang tính
cấu trúc chứ không phải theo từng recipe: nó đúng cả với `cargo test` trần, và không thể bị
quên khi ai đó thêm recipe mới. Thứ nó **không** thấy được là biến export trong shell
profile — trong tiến trình, nó không phân biệt được với biến người vận hành tự gõ. Nên các
recipe mang nghĩa "đã chạy hết" còn kèm `{{ no_db_skip }}` (`env -u EGRET_ALLOW_DB_SKIP`),
và `crates/egret-test-db/tests/every_test_recipe_refuses_the_skip.rs` sẽ đỏ nếu một recipe
trong `justfile` chạy `cargo test` mà không xử lý biến này.

## Chạy kèm database (giống CI)

```bash
just up                                                          # Postgres ở :5432
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test --workspace
```

> **Không bao giờ trỏ `DATABASE_URL` vào database dev `egret`.** `#[sqlx::test]` đọc biến
> này qua dotenvy **vô điều kiện**, nên một biến *không export* không làm test skip — nó
> làm test đọc `.env`, vốn trỏ vào database dev, và tạo schema `_sqlx_test` ngay bên trong.

### Mọi binary có `#[sqlx::test]` đều mang guard đó

Không phải quy ước hình thức — đây là lỗ hổng đã đo được. Với `DATABASE_URL` không đặt,
`egret-automation/tests/report_tick.rs` (7 test) và `tick_end_to_end.rs` (11 test) đã **đạt**,
sau khi ghi scratch database vào chính `egret` dev: `_sqlx_test.databases` bên trong nó cho
`n_tup_ins = 338`. Guard đã tồn tại — ở 5 trên 24 file cần nó.

Bây giờ đủ 24, mỗi **binary** một cái (guard phải ở cùng tiến trình với test nó bảo vệ):
mỗi `tests/<name>.rs` có guard riêng, và bốn crate có `#[sqlx::test]` trong `src/` mang một
module `dotenv_fallback_guard` trong `src/lib.rs` cho binary `--lib`.
`crates/egret-test-db/tests/every_sqlx_test_binary_guards_the_dotenv_fallback.rs` quét cây
mã nguồn và đỏ, nêu tên file, khi một binary dùng `#[sqlx::test]` mà không có guard.

Đây là **phát hiện**, không phải **ngăn chặn**: libtest không bảo đảm thứ tự, nên lần chạy
làm guard đỏ vẫn đã kịp tạo scratch database. Ngăn chặn thật sự cần một hook chạy trước mọi
test trong tiến trình; sqlx không có, `[env]` trong `.cargo/config.toml` sẽ đổi luôn
`cargo run -p egret-bin` (server dev), còn constructor pre-main chỉ chạy nếu linker giữ lại
crate mà không gì tham chiếu. Đỏ-và-ồn tốt hơn xanh-và-im.

## CI

`.github/workflows/ci.yml` đã chạy service `postgres:16` và đặt `DATABASE_URL` ở mức
**workflow** (khối `env:` ở đầu file, nên mọi job đều thừa hưởng),
nên **thay đổi này không làm CI khác đi**. Nó chỉ đổi hành vi trên máy không có database.

## Trục STORE: `just check` cấp store, chứ không bỏ qua nó

`crates/egret-dataapi/tests/serving_shared_store.rs` (14 test) **panic** khi thiếu
`EGRET_TEST_SHARED_STORE_URL`, cùng một kỷ luật như trên. `EGRET_ALLOW_STORE_SKIP=1` là
opt-out.

**GitHub CI hết quota, nên `just check` LÀ cổng.** Mà `check` từng quyết ba trục DB /
ENGINE / DOCKER và để trục này **chưa quyết**: lần chạy workspace kết thúc
`0 passed; 14 failed`, và chính dòng panic đó khuyên người đọc export
`EGRET_ALLOW_STORE_SKIP=1`. Export một lần là từ đó mọi `just check` bỏ qua cả 14 test,
với `eprintln!` bị libtest nuốt. Một recipe tự buộc người dùng tắt một phần của chính nó
còn tệ hơn một recipe quên cổng.

SP4d Task 8 quyết trục này bằng cách **CHẠY** suite:

* `valkey` nằm trong `deploy/docker-compose.dev.yml` ở cổng **6399**, nên `just up` cấp nó
  cạnh Postgres mà `check` vốn đã đòi. Không có Docker thì
  `brew install valkey && valkey-server --port 6399` là đủ — suite cần một server giao thức
  redis, không cần daemon.
* `check` cấp `EGRET_TEST_SHARED_STORE_URL` **và** chà sạch opt-out
  (`no_store_skip`), nên một biến export trong shell profile không thể làm nó rỗng.
* `just test-no-db` đặt `EGRET_ALLOW_STORE_SKIP=1` **ngay trên dòng**, và cả hai recipe in
  ra cuối terminal những gì màu xanh đó phủ và không phủ.
* `just test-dataapi-shared-store` chạy riêng suite này với `--nocapture`.
* `crates/egret-test-db/tests/every_test_recipe_refuses_the_skip.rs` kiểm bằng máy: recipe
  chạy `cargo test` mà không nêu target và không quyết biến này thì guard đó đỏ, kèm số dòng.

## Test cần ENGINE là trục riêng

Test cần một query engine sống vẫn tự bỏ qua, vì CI không có engine:
`EGRET_TEST_STARROCKS_URL`, `EGRET_TEST_TRINO_URL`, `EGRET_TEST_S3_ENDPOINT`,
`EGRET_TEST_RANGER_E2E`. Xem `just test-dataapi-starrocks` cho đường Data API duy nhất
chạy trên engine thật thay vì `QueryExecPort` giả.
