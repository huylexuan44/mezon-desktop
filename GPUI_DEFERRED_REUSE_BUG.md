# Bug: crash "Exceeded maximum (10) deferred depth" (gpui vendored 0.2.2)

Ghi lại 2026-07-30. Trạng thái: **đã workaround ở code owned, bug gốc trong vendor vẫn còn**.

## Triệu chứng

```
ERROR mezon: panic at crates/vendor/gpui/src/window.rs:2961:13: Exceeded maximum (10) deferred depth
```

- Repro thật (Linux, intermittent): member list → mở profile popover của member → bấm **Add Role** → sometimes crash.
- Cùng chữ ký với vụ crash cũ khi thêm `deferred().with_priority(1000)` cho toast:
  `key_dispatch.rs: node N was not part of the reused subtree` — **cùng một root cause**, khác chỗ chết.

## Điều kiện trigger (cần đủ 3)

1. Một overlay `deferred()` mà **bên trong nội dung của nó lại có một `deferred()` nữa** (lồng 2 tầng).
   Hiện chỉ có một component như vậy: `UserProfilePopover` — panel Add Role là
   `deferred(...)` bên trong popover (`user_profile_popover.rs`, nhánh `add_role_open`).
2. Overlay đó được đăng ký từ trong một subtree có ancestor **`.cached()`**:
   - member panel — cached ở `chat/area.rs`
   - timeline (message list) — cached ở `chat/area.rs`; popover mở từ click avatar trong chat
     (`ChannelMessages::mention_popover`) đi đường này.
3. Một frame mà subtree cached đó **không dirty** (cache hit) trong khi có view khác dirty
   (tin nhắn tới, typing, presence...). Vì cần trùng 3 điều kiện nên bug mang tính "sometimes".

## Cơ chế (đọc từ vendor source, upstream Zed hiện tại y hệt)

- `Window::prepaint_deferred_draws` xử lý deferred draw theo **vòng** để hỗ trợ lồng nhau;
  mỗi vòng nó `mem::take` toàn bộ `next_frame.deferred_draws` ra xử lý.
- `prepaint_index()` đo `deferred_draws_index = next_frame.deferred_draws.len()` — vì vec vừa bị
  take nên **index ghi trong lúc chạy vòng là index cục bộ của vòng, luôn đếm lại từ 0**.
- Cuối frame các vòng được nối thành một mảng phẳng. Frame sau, view `.cached()` hit cache thì
  `reuse_prepaint` **cắt slice mảng phẳng bằng đúng những index cục bộ đó** → cắt sai chỗ.
  Trường hợp xấu: overlay được reuse cắt trúng **chính nó** → tự đăng ký lại mỗi vòng →
  đủ 10 vòng → panic.
- Mọi index khác trong `PrepaintStateIndex` (dispatch tree, hitboxes, tooltips...) đều flat —
  chỉ mình `deferred_draws_index` bị round-local. Đây là bug thật của upstream, không phải
  thiết kế: `reuse_prepaint` còn clone `prepaint_range` cẩn thận cho từng draw được reuse,
  chứng tỏ tác giả muốn chuỗi reuse lồng nhau chạy được.
- Vì sao upstream viết vậy: `mem::take` từng vòng là cách sạch nhất để né borrow conflict
  (prepaint một element có thể push deferred draw mới vào chính vec đang duyệt). Đúng trong
  phạm vi vòng lặp; sai khi giao với view caching. Zed không dính vì họ hầu như không có
  tổ hợp cached × nested-deferred; mezon dính vì dựa nặng vào `.cached()` cho perf.

## Fix đã áp dụng (owned code, không đụng vendor)

Chặn điều kiện 3: **đang mở popover thì tắt `.cached()`** ở 2 chỗ host — không cache hit thì
range hỏng không bao giờ được tiêu thụ:

- `chat/area.rs` — gate cả timeline lẫn member panel bằng 2 accessor mới:
  - `ChannelMessages::profile_popover_open()` (`mention_popover.is_some()`)
  - `MemberListPanel::profile_popover_open()` (`open_profile.is_some()`)

Chi phí: trong lúc popover mở, 2 panel đó render lại mỗi frame thay vì replay cache
(trạng thái tương tác ngắn, chấp nhận được). Popover đóng → cache hoạt động lại bình thường.

## Còn hở gì

- **Bug gốc trong vendor vẫn nguyên.** Guard rule: *không lồng `deferred()` trong nội dung một
  `deferred()` khác khi có ancestor `.cached()`* — nếu bắt buộc, gate cache của ancestor y như trên.
- Đã audit các site deferred hiện có: user info bar (2 deferred **sibling**, ancestor không cached),
  clan-rail context menu (1 tầng, submenu inline), role color/icon picker + audit-log DatePicker
  dưới clan settings (đều 1 tầng) → hiện an toàn. `UserProfilePopover` là component duy nhất lồng.
- `date_picker.rs` vẫn còn `.with_priority(1)` pre-existing — đừng thêm priority mới
  (xem vụ toast phải revert).

## Fix tận gốc (nếu sau này muốn — đã viết và verify compile rồi revert theo quyết định không sửa vendor)

Vendor edit trong `prepaint_deferred_draws`: bỏ `mem::take` từng vòng, xử lý tại chỗ trên vec
phẳng theo range `round_start..round_end` (item lồng tự append thành vòng kế); né borrow bằng
`element.take()` + copy field nhỏ ra local + write-back sau prepaint. Index ghi ra tự nhiên là
flat → reuse cắt đúng. Giữ nguyên: thứ tự mảng cuối, sort priority trong vòng, paint phase,
cap depth 10. Sửa luôn cả class `with_priority` và view `.cached()` nằm trong nội dung deferred.
Nhớ: là vendor edit thì phải re-apply khi bump snapshot gpui.

## Verify

- Workaround: clippy/fmt clean, `cargo test -p mezon-ui` pass, build ok.
- Chưa test runtime flow thật trên Linux — cần thao tác lại: popover member → Add Role,
  và click avatar trong chat → Add Role, để popover mở một lúc trong lúc chat có traffic.
