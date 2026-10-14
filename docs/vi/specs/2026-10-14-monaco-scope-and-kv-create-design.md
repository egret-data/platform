# Việc tiếp theo của W7d: phím tắt Monaco gắn với editor của nó, và tạo key KV có thể từ chối ghi đè

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-14-monaco-scope-and-kv-create-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** đã hiện thực, 2026-10-14.

**Phụ thuộc:** W7d (#669), gói đã gắn Ctrl/Cmd+S vào editor của nó và để lại hai việc dưới đây; W9c (#654), kho KV và các route của nó.

Bộ rule frontend `docs/en/frontend-rules.md` ràng buộc mọi dòng code frontend.

## 1. Những gì đã kiểm trong code

Mỗi tiền đề của brief được đọc trong code trước khi viết gì.

| Khẳng định | Ở đâu | Đúng? |
|---|---|---|
| `addCommand` của Monaco standalone đăng ký một keybinding trong keybinding service dùng chung, bỏ disposable đi, và không gắn với editor | `monaco-editor` 0.56.0, `esm/vs/editor/standalone/browser/standaloneCodeEditor.js:85-94`: `addDynamicKeybinding(commandId, keybinding, handler, whenExpression)`, disposable bị bỏ và trả về id của command, `when` chỉ là `context` của bên gọi. `addAction` (`:98-148`): `precondition = editorId == this.getId()` và `when` của keybinding có nó (`:109`, `:111`), và trả về `DisposableStore` | Đúng |
| Resolver trả về keybinding khớp được đăng ký gần nhất | `esm/vs/platform/keybinding/common/keybindingResolver.js:281-290`, `_findCommand` duyệt các match từ cuối | Đúng |
| Hai chỗ Ctrl/Cmd+S của W7d dùng `addAction` và dispose khi unmount | `frontend/src/components/workflows/editor/CodeTab.tsx:62`, `frontend/src/components/workflows/namespaces/files/NamespaceFileEditor.tsx:164` | Đúng |
| Các chỗ `addCommand` còn lại | `frontend/src/components/sqllab/SqlEditor.tsx:150` và `frontend/src/routes/explore/dataapi/detail/Query.tsx:760`, cả hai `CtrlCmd \| Enter`. Không có `addCommand(` nào khác dưới `frontend/src` (grep) | Đúng, đúng hai chỗ |
| PUT của KV luôn ghi đè | `crates/egret-workflow/src/storage/kv/routes.rs:656` (`overwrite: true`) | Đúng |
| Kho hỗ trợ `overwrite: false`, từ chối một hàng mới nhất còn sống bằng `Conflict` | `crates/egret-workflow/src/storage/kv/store.rs:1145`, dưới lock của key, xét bằng `KvEntry::live` (`store.rs:112`: không bị xoá mềm và không hết hạn; `expired` đọc ở `clock_timestamp()` qua `wcols!`, `store.rs:131-135`) | Đúng |
| Lời từ chối "busy" của kho cũng là một `Conflict` | `store.rs:833` (`busy`) | Đúng: API không phân biệt được hai trường hợp theo variant, nên cần thay đổi ở kho trong §3.1 |
| `kvApi.ts` truyền header qua fetcher được sinh | `frontend/src/components/workflows/namespaces/kv/kvApi.ts` `putKvValue`: `workflowPutKvValue(ws, namespace, key, body, { headers })` | Đúng |
| Drawer tạo key đã hỏi trước khi ghi đè một key mà DANH SÁCH đã hiện | `KvEditDrawer.tsx` `save()`: `existingKeys.has(form.key)` mở "Overwrite “X”?" | Đúng; lỗ hổng là một key được tạo sau khi danh sách được đọc |

## 2. Việc 1: mọi phím tắt Monaco là một action của editor

Hai `addCommand(CtrlCmd | Enter)` thành `editor.addAction({ id, label, keybindings: [CtrlCmd | Enter], run })`, và disposable được dispose khi component unmount và khi editor của nó bị thay (một `onMount` thứ hai), như W7d đã làm cho Ctrl/Cmd+S.

- `SqlEditor`: action `egret.sql.run` ("Run SQL"), disposable của nó được giữ cùng completion provider và listener selection mà component vốn đã dispose. Chốt chặn read-only (`onRunRef`) không đổi.
- Editor script của `Query.tsx`: action `egret.dataapi.preview` ("Run the preview"), giữ trong một ref được dispose khi unmount và trước khi một lần mount lại đăng ký lần nữa. Chốt chặn `trashedRef` không đổi.

Vì sao: một keybinding `addCommand` trả lời trong bất kỳ editor nào đang có focus, lần đăng ký mới nhất thắng, và nó không bao giờ bị gỡ. SQL Lab với hai tab, hoặc trang Data API chuyển giữa SQL và script, sẽ chạy handler của nhầm editor, kể cả một editor đã unmount.

**Test** (jsdom, các stub Monaco sẵn có của từng suite được thêm `getId` và `addAction`, và `addCommand` bị BỎ khỏi stub, để một lần thoái lui sẽ ném lỗi): action của hai editor mỗi cái chỉ chạy handler của chính nó; unmount dispose action; `SqlEditor`, `Query`, `Statements` và `EndpointCreate` giữ các assertion Ctrl/Cmd+Enter sẵn có, qua action.

## 3. Việc 2: `If-None-Match: *` trên `PUT …/kv/{key}`

### 3.1 Backend

- `put_value` đọc `If-None-Match`. Không có: `overwrite: true`, như hiện nay. Đúng `*` (bỏ khoảng trắng, ở mọi lần xuất hiện): `overwrite: false`. Giá trị khác: 400 `If-None-Match on a KV write takes only "*"`, trước khi đọc body. KV không có ETag, nên một danh sách entity-tag chỉ có thể bị hiểu sai. Đó là lý do KV dùng `If-None-Match` còn nsfiles dùng `if_revision`: KV không có revision để so sánh, nên điều kiện tiên quyết duy nhất của nó là "chưa tồn tại".
- Kho có thêm `try_set`, trả lời một lần ghi không-ghi-đè bị từ chối là `Ok(Err(live_entry))`; `set` giữ như cũ (nó đổi trường hợp đó thành thông điệp `Conflict` của Kestra, nên task `kv.Set` với `overwrite: false` không đổi). Route đổi `Err(entry)` thành **409** với `code: "kv_exists"` và thông điệp `conflict: key 'X' already exists in namespace 'N' (revision R); nothing was written`. Không có gì được lưu ngoài object staging mà kho quên đi, không hàng nào được ghi, không audit nào được phát.
- **Tính còn sống là của chính kho:** key tồn tại chỉ khi hàng mới nhất của nó còn sống, `KvEntry::live`, xét dưới lock của key ở `clock_timestamp()`. Một key có revision mới nhất đã xoá mềm hoặc đã hết hạn thì KHÔNG tồn tại: lần tạo thành công và ghi revision kế tiếp (`max + 1`, nên lịch sử được giữ), đúng như GET trả 404 cho nó.
- 409 chứ không phải 412 của RFC 9110: các lời từ chối "thứ đó đã đổi" khác của API là 409 kèm `code` (`file_changed`, `draft_changed`), và client rẽ nhánh theo `code`. 409 busy (không có `code`) không đổi.
- openapi: header là một tham số có tài liệu của PUT, dòng 409 nêu cả hai trường hợp. `openapi.json` và client được sinh lại.

**Test** (`crates/egret-workflow/tests/kv_api.rs`, `MemStore` của binary): tạo một key mới thành công; tạo một key còn sống là 409 `kv_exists` và bảng hàng cùng các object không đổi; tạo đè lên một key đã xoá mềm và một key đã hết hạn thành công như revision kế tiếp; không có header thì một key còn sống bị ghi đè như trước; `If-None-Match` sai dạng là 400 và không ghi gì.

### 3.2 Frontend

- `putKvValue` nhận `onlyIfAbsent`; khi bật, nó thêm header `If-None-Match: *` (vẫn là `workflowPutKvValue` được sinh, qua `customInstance`, FE-DATA-2).
- Drawer tạo key gửi header khi tạo một key mà danh sách không hiện. Nó không gửi khi sửa, cũng không gửi sau khi người dùng đã xác nhận "Overwrite" cho một key danh sách đã hiện.
- Một 409 có `code` là `kv_exists` mở "A key named X was created meanwhile — overwrite?"; Overwrite gửi lại cùng body không kèm header; Cancel giữ nguyên form như đã gõ (FE-ACTION-3).

**Test** (`NamespaceKvTab.test.tsx`, server giả trả 409 `kv_exists` cho một PUT mang header): header chỉ có khi tạo; không có khi sửa; không có sau Overwrite dựa trên danh sách; dialog 409, Overwrite gửi lại không kèm header, Cancel không gửi gì thêm.

## 4. Bằng chứng test cắn

Mỗi việc một mutation, mỗi cái chạy với test của nó rồi hoàn lại:

- Việc 1: `SqlEditor` quay về `addCommand`.
- Việc 2: route bỏ qua header (`overwrite: true` luôn luôn).
