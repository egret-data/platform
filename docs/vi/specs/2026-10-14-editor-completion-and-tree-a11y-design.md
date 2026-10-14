# Việc tiếp theo: gợi ý SQL gắn với editor của nó, và ngữ nghĩa cây cho danh sách namespace

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-14-editor-completion-and-tree-a11y-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-14.

**Phụ thuộc:** W7d (#669), gói đã thêm danh sách namespace và cây Files của namespace; #670, gói đã gắn phím tắt Monaco với editor của nó và để nguyên phần gợi ý SQL.

Bộ rule frontend `docs/en/frontend-rules.md` ràng buộc mọi dòng code frontend.

## 1. Những gì đã kiểm trong code

Mỗi tiền đề của brief được đọc trong code trước khi viết gì.

| Khẳng định | Ở đâu | Đúng không? |
|---|---|---|
| `SqlEditor` đăng ký một completion provider cho ngôn ngữ `sql` mỗi lần mount | `frontend/src/components/sqllab/SqlEditor.tsx:137`, `monaco.languages.registerCompletionItemProvider("sql", …)`, được dispose khi unmount và khi mount lần hai | Đúng |
| Monaco giữ provider theo ngôn ngữ và hỏi mọi provider cùng điểm, gộp các câu trả lời | `monaco-editor` 0.56.0: `registerCompletionItemProvider` đăng ký vào `ILanguageFeaturesService.completionProvider` (`esm/vs/editor/standalone/browser/standaloneLanguages.js:448`); `provideSuggestionItems` hỏi từng provider trong `registry.orderedGroups(model)` và gộp mọi câu trả lời (`esm/vs/editor/contrib/suggest/browser/suggest.js:185-211`). Hai provider đăng ký cho `"sql"` có cùng điểm, nên cả hai trả lời cho mọi model SQL | Đúng |
| Khi mở hai tab SQL Lab, mỗi editor gợi ý schema của cả hai tab | `frontend/src/routes/explore/SqlLab.tsx:61` chỉ render tab ĐANG MỞ (`<SqlTab key={active.id} …>`); editor của tab kia bị unmount và provider của nó bị dispose. Các trang Data API và trang dataset cũng chỉ render một `SqlEditor` mỗi lúc | **Không, hiện tại thì không.** Lỗi đang tiềm ẩn: nó lộ ra ngay khi hai `SqlEditor` được mount cùng lúc (mỗi cái sẽ trả lời bằng schema của mình và bản keyword của mình cho cả hai model). Vẫn sửa vì rẻ và component này dùng chung |
| Các đăng ký toàn cục theo ngôn ngữ khác (`registerHoverProvider` và tương tự) trong `frontend/src` | `grep -rnE "languages\.(register\|set)[A-Za-z]*\(" frontend/src` chỉ thấy `SqlEditor.tsx:137`. Gợi ý và hover YAML của editor flow đến từ `monaco-yaml` (`frontend/src/components/workflows/editor/monacoYaml.ts`): MỘT instance cho cả app, cấu hình theo ETag của schema và chỉ áp cho model flow qua `fileMatch: [FLOW_MODEL_GLOB]`. Nó cố ý là một cho cả app (cấu hình lần hai sẽ chồng thêm một instance không tắt được), không phải một cho mỗi editor | Chỉ `SqlEditor` là theo từng editor |
| Danh sách namespace không có ngữ nghĩa cây | `frontend/src/routes/workflows/NamespacesList.tsx`: một `ui/table` có hàng thụt lề theo `data-depth` và padding; không `role`, không level, không có gì để mở hay gập, và link của mỗi hàng là một điểm dừng Tab riêng | Đúng |
| Cây Files của namespace thiếu ngữ nghĩa cây | `frontend/src/components/workflows/namespaces/files/FileTree.tsx:225-301`: `role="tree"`, `role="treeitem"` với `aria-level`, `aria-expanded`, `aria-selected`, `tabIndex` luân phiên, Up/Down, Right, Left, Home/End, Enter/Space, F2, Delete, Shift+F10; được test trong `NamespaceFilesTab.test.tsx` | **Không.** Nó đã có sẵn. Không đổi (§4) |

## 2. Gợi ý SQL

**Mỗi provider chỉ trả lời cho model của editor của nó.** Provider của `SqlEditor` so model mà Monaco đưa vào với `editor.getModel()`, đọc lúc được gọi, và trả `{ suggestions: [] }` cho mọi model khác. Đọc lúc được gọi chứ không giữ từ lúc mount, để một model bị thay dưới editor (`path` của `@monaco-editor/react`) vẫn được trả lời.

Đã cân nhắc và không chọn: một provider dùng chung, phân phối theo URI của model tới một registry. Cách đó bỏ được hình dạng N provider cho một ngôn ngữ, nhưng thêm state ở mức module phải giữ khớp với mount và unmount; kiểm tra theo model giữ mọi state trong component vốn đã dispose nó.

Dispose không đổi: provider được dispose khi unmount và khi `onMount` chạy lại.

**Test** (`frontend/src/components/sqllab/__tests__/SqlEditor.test.tsx`): Monaco giả ghi lại mọi provider cùng ngôn ngữ của nó và việc nó đã bị dispose chưa, và mỗi editor giả có model riêng. Mount hai `SqlEditor` với hai schema khác nhau; hỏi mọi provider `sql` còn sống cho từng model, như Monaco làm, thì editor thứ nhất nhận bảng của mình chứ không nhận bảng của editor thứ hai, và ngược lại, và mỗi keyword chỉ một lần. Một test nữa kiểm dispose khi unmount.

## 3. Danh sách namespace thành treegrid

Danh sách giữ bảng của nó (FE-PAGE-4) và trở thành một **treegrid** của WAI-ARIA, mẫu dành cho bảng có các hàng tạo thành cây:

- Bảng nhận `role="treegrid"`, `aria-label` là "Namespaces", và caption làm phần mô tả (`aria-describedby`), cùng một dòng chỉ dành cho trình đọc màn hình nói phím nào dùng được.
- Mỗi hàng thân bảng nhận focus được và mang `aria-level` (độ sâu + 1), `aria-setsize` và `aria-posinset` trong các anh em của nó, và `aria-expanded` khi nó có con. Một lá không có `aria-expanded`.
- Các ô là `role="gridcell"`; các ô của hàng tiêu đề vẫn là tiêu đề cột.
- **Một điểm dừng Tab, luân phiên:** hàng đang có focus khi nó còn hiện, nếu không thì hàng đầu. Link của mỗi hàng không nhận điểm dừng Tab (`tabIndex={-1}`); Enter trên hàng đi theo link đó.
- Phím trên một hàng: Down và Up sang hàng hiện kế tiếp và trước đó; Home và End tới hàng đầu và cuối; Right mở một hàng đang gập, và trên một hàng đang mở thì sang con đầu tiên; Left gập một hàng đang mở, và nếu không thì sang hàng cha; Enter mở namespace.
- **Gập là tính năng mới.** Một hàng có con hiện một nút mũi tên để gập và mở nó, cho chuột; nút không nhận điểm dừng Tab, vì phím mũi tên làm cùng việc. Mọi thứ bắt đầu ở trạng thái mở, như hiện nay. Tập đã gập chỉ sống trong bộ nhớ: đó không phải một lựa chọn ai muốn chia sẻ (FE-PAGE-2) và cũng không phải một tuỳ chọn được lưu. Một hàng đã gập ẩn mọi hàng bên dưới nó; phần tóm tắt vẫn đếm các namespace đã lọc.

Chỉ focus theo hàng: các ô chứa số, nên di chuyển giữa các ô chỉ thêm phím mà không thêm gì để đọc.

**Test** (`frontend/src/routes/workflows/__tests__/NamespacesList.test.tsx`): role, level, set size, vị trí và `aria-expanded` trên fixture chưa sắp xếp; một điểm dừng Tab; Down, Up, Home, End; Right mở rồi đi vào, Left gập rồi về cha; Enter điều hướng; nút mũi tên gập cho chuột.

## 4. Không đổi

- Cây Files đã hiện thực mẫu tree (§1). Hai khoảng trống nhỏ được ghi lại làm việc tiếp theo thay vì sửa ở đây: nút menu của các hàng là một phần tử tương tác nằm trong một `treeitem` (nó không nhận điểm dừng Tab, nên chỉ dành cho chuột), và cây không đặt `aria-setsize`/`aria-posinset` (không cần khi mọi anh em đều có trong DOM, và đúng là vậy: một thư mục liệt kê mọi con của nó).
- `monaco-yaml` (§1).

## 5. Bằng chứng test cắn

Mỗi mục một mutation, mỗi mutation chạy với các test của nó rồi hoàn lại; kết quả được ghi trong PR:

- Mục 1: bỏ phép kiểm model khỏi provider.
- Mục 2: bỏ `aria-level` của hàng, và riêng một lần, bỏ việc phím Left sang hàng cha.
