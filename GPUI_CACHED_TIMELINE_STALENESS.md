# Bug: timeline bọc `.cached()` giữ avatar/nickname cũ cho tới khi có notify khác

Ghi lại 2026-08-14 trong lúc review PR #330 (topic message paging). Trạng thái: **chưa sửa, có sẵn từ trước PR đó, dính cả channel lẫn topic**.

## Cơ chế

`AnyView::cached(...)` chỉ dựng lại subtree khi **chính entity đó** được `cx.notify()`. Cache hit thì gpui tái dùng layout/prepaint/paint của frame trước (`window.rs::reuse_prepaint` / `reuse_paint`), nên state đã đổi trong store mà không có notify tới đúng view thì **pixel cũ vẫn nằm nguyên trên màn hình**.

Hai timeline đều được mount kiểu này:

- channel: `crates/mezon-ui/src/chat/area.rs:669`
- topic panel: `crates/mezon-ui/src/chat/create_topic_panel.rs:290`

(Không gỡ được `.cached()` ở panel topic: chính `StyleRefinement::default().size_full()` trong đó là thứ cấp chiều cao cho view — bỏ ra thì list chỉ cao ~1 dòng và panel trông như trắng trơn. Đã thử cả `div().size_full().child(...)` lẫn `AnyView::from(...)`, không thay thế được.)

## Chỗ hụt notify

Trong `ChannelMessages::new` (`crates/mezon-ui/src/chat/message/channel_messages.rs`), 3 observer sau **xoá memo render rồi có thể không notify**:

| Observer | Dòng | Hành vi |
|---|---|---|
| `ClanMembersStore` | ~1422 | xoá `memo.avatars` / `display_names` / `role_styles`, rồi chỉ `cx.notify()` khi `refresh_derived_state` trả `true` |
| `UsersByUserStore` | ~1452 | chỉ chạy khi `is_dm()` → clan channel/topic không bao giờ notify |
| `GroupMembersStore` | ~1465 | như trên |

`refresh_derived_state` (dòng ~3059) tính welcome/onboarding/unread-boundary/FAB — ở panel topic các giá trị này gần như luôn `None` và không đổi ⇒ **`cx.notify()` không chạy** ⇒ cache không bị invalidate ⇒ tên/avatar hiển thị cũ.

Với channel thì ít lộ hơn vì có nhiều nguồn notify khác (tin mới, typing, presence...) nhưng bản chất giống hệt: memo đã bị xoá mà view không được vẽ lại.

## Repro (chưa làm, cần 2 tài khoản)

1. A mở panel topic (hoặc channel) đang có tin của B.
2. B đổi **nickname trong clan** hoặc **avatar**.
3. A không chạm gì vào cửa sổ.
4. Kỳ vọng: dòng tin của B đổi tên/avatar. Thực tế (dự đoán theo code): giữ nguyên cho tới khi có event topic khác, hoặc A cuộn, hoặc A rê chuột vào panel (`.on_hover` thêm ở PR #330 tạo đường thoát này).

## Bảng nguồn notify của topic box (đã rà khi review PR #330)

| Sự kiện | Notify? |
|---|---|
| Tin mới / sửa / xoá / reaction trong topic | ✅ `TopicUpdated` / `Updated{id}` → `on_topic_store_event` |
| Sửa/reaction lên chính tin gốc (origin) | ✅ `id` nằm trong `topic_row_ids` |
| Tin mới ở channel | ❌ đúng thiết kế — list topic không hiển thị tin channel |
| Đổi ngôn ngữ (settings) | ✅ notify vô điều kiện |
| Đổi role của clan đang mở | ✅ |
| Emoji recent đổi | ✅ khi khác |
| `channel_list` / `clan_list` | ❌ đi qua `reconcile_cold` (dòng ~3014) mà hàm này return sớm cho topic box — chỉ chi phối header/skeleton của channel nên không ảnh hưởng nội dung |
| **Đổi avatar / nickname thành viên** | ❌ **đây là lỗ hổng** |

## Hướng sửa đề xuất

Khi observer đã xoá memo avatar/display name/role style thì notify luôn, thay vì phụ thuộc `refresh_derived_state`:

```rust
// clan_members_observe
let cleared = { ...clear memo...; true };
if cleared || this.refresh_derived_state(cx) { cx.notify(); }
```

Và bỏ điều kiện `is_dm()` ở `users_by_user` / `group_members` (hoặc đổi thành: notify khi memo thực sự có entry bị xoá).

**Cần đo trước khi merge**: tần suất notify của 2 timeline sau thay đổi. `ClanMembersStore` / `UsersByUserStore` có thể bắn dày lúc mới vào clan (fetch member list, presence); notify vô điều kiện ở đó sẽ kéo theo dựng lại cả subtree message list. Cách đo đã dùng ở PR #330: cắm `tracing::debug!` đếm số lần render + `Instant` quanh `render()` của timeline, mở clan đông thành viên rồi đếm; ngưỡng tham chiếu là `render_topic_box` p50 ≈ 33 µs, `TopicPanel::render` p50 ≈ 16 µs (debug build, frame 16 600 µs).

Nếu tần suất quá dày thì giải pháp thay thế là coalesce: đánh dấu `memo_dirty` rồi notify một lần ở frame kế (`cx.on_next_frame` **không dùng được** — xem ghi chú dưới), hoặc dùng debounce bằng `cx.spawn` + timer ngắn.

## Ghi chú kèm theo (rút ra khi làm PR #330)

- `cx.on_next_frame` **không đáng tin** cho view nằm trong subtree cached: callback chỉ chạy khi window thật sự vẽ frame, mà cache hit thì không có frame nào ⇒ cờ "đã schedule" kẹt vĩnh viễn (đã gặp với `pagination_check_scheduled` của topic box). Muốn chạy định kỳ thì đặt trong `render()` của chính view đó.
- Đọc `list_state` **trong scroll handler của `gpui::list`** là panic: `RefCell already mutably borrowed` (`gpui/src/elements/list.rs:766`) — handler được gọi khi RefCell đang mượn mut.
- Wheel event chỉ tới list khi hitbox nằm trong `window.mouse_hit_test`, mà cái này dựng từ **frame đã vẽ gần nhất** ⇒ subtree cached vừa mount thì cú cuộn đầu bị nuốt. PR #330 vá bằng `.on_hover` → notify 1 lần khi con trỏ vào panel.
