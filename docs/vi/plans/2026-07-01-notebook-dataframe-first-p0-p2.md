# Notebook dataframe-first — P0 spike + P1 backend token + P2 helper Iceberg/Trino (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-01-notebook-dataframe-first-p0-p2.md`. File này rút gọn — code/step đầy đủ xem bản Anh.
> Spec: `docs/en/specs/2026-07-01-notebook-dataframe-first-daft-design.md`. Thực thi qua superpowers:subagent-driven-development.

**Mục tiêu:** de-risk StarRocks-over-JWT (spike), rồi ship helper Daft-direct `iceberg()` + `trino()` dựa trên endpoint mint JWT per-user mới — user notebook nhận Daft DataFrame lazy as-self.

**Phạm vi (decompose):** plan này = P0 + P1 + nửa Iceberg/Trino của P2. `starrocks()` (transport tuỳ P0), gỡ SQL cell surface (P3), verify live đầy đủ (P4) → **plan follow-up** sau khi P0 có kết quả (không gỡ path cũ trước khi helper mới chứng minh live).

## Task
- **T1 (P0 spike)** — chứng minh Python/Daft→StarRocks bằng JWT (security integration + JWT tester). Thử: (A) Daft read_sql qua SQLAlchemy MySQL, (B) mysql client 9.2+ token-file, (C) N/A. Viết `docs/en/specs/...starrocks-jwt-spike-findings.md` + khuyến nghị transport (direct/hybrid). Commit findings.
- **T2 (P1)** — seam `EngineConnResolver` (`crates/egret-kernel/src/engine_conn.rs`): `database_id → EngineConn{kind,host,port}` + fake, mirror `NotebookQueryRunner`. Unit test.
- **T3 (P1)** — handler `POST /kernel/engine-token` (`routes.rs`): `KernelIdentity` → validate engine enabled (`list_engine_options`) → resolve coords → mint JWT (`EngineTokenMinter`) → `{jwt,expires_at,kind,host,port}`. Test enabled → 200, disabled → 404. Reuse pattern `kernel_query`.
- **T4 (P1)** — impl cụ thể `EngineConnResolverImpl` trong egret-bin qua `DatabaseRepo::get` (`Database{engine,host,port}`), inject vào `KernelState`.
- **T5 (P1)** — thêm `host,port` vào `EngineOption` + `list_engine_options` (store.rs) → manifest serialize coords.
- **T6 (P2)** — `engines.py::_engine_token(engine)` fetch+cache JWT/coords, refresh gần hết hạn (mirror `query.py`, không silent-empty). pytest.
- **T7 (P2)** — `trino(sql)` = `daft.read_sql(sql, factory)` với SQLAlchemy `trino://` + `JWTAuthentication(jwt)`. pytest (factory là callable).
- **T8 (P2)** — `iceberg(table)` = `daft.read_iceberg(catalog().load_table(table))`. pytest.
- **T9 (P2)** — export + auto-load `iceberg/trino` trong `__init__.py` + `startup/00-egret.py`. test_init.
- **T10** — verify: fmt/clippy/test backend + pytest kernel-helpers + smoke tay (iceberg/trino render Daft DF as-user); finish branch.

## Rủi ro
- P0 chỉ gate follow-up, không gate plan này (iceberg/trino không phụ thuộc spike).
- Trino `http_scheme` hard-code `https`; nếu dev Trino plaintext → thread `scheme` từ coords (thêm vào `EngineConn`).
- `daft.read_sql` factory phải trả connection sống mỗi lần gọi (Daft có thể mở nhiều lần) — factory đọc lại token cached.
- Chưa gỡ path SQL cũ (`%%egret_sql` vẫn còn) → helper additive, không vỡ gì giữa chừng.
