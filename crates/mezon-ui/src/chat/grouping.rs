use mezon_store::{Message, message_combined_with_prev};

pub fn is_combined(prev: Option<&Message>, msg: &Message) -> bool {
    message_combined_with_prev(prev, msg)
}

#[cfg(test)]
mod tests {
    use mezon_store::MessageId;

    use super::*;

    fn make_msg(id: i64, sender: &str, time: i64) -> Message {
        Message::new(MessageId(id), "", sender, sender, time)
    }

    #[test]
    fn same_sender_within_window_combines() {
        let prev = make_msg(1, "alice", 1000);
        let msg = make_msg(2, "alice", 1100);
        assert!(is_combined(Some(&prev), &msg));
    }

    #[test]
    fn different_sender_breaks_group() {
        let prev = make_msg(1, "alice", 1000);
        let msg = make_msg(2, "bob", 1100);
        assert!(!is_combined(Some(&prev), &msg));
    }

    #[test]
    fn time_window_expires_breaks_group() {
        let prev = make_msg(1, "alice", 1000);
        let msg = make_msg(2, "alice", 1700);
        assert!(!is_combined(Some(&prev), &msg));
    }

    #[test]
    fn first_message_is_never_combined() {
        let msg = make_msg(1, "alice", 1000);
        assert!(!is_combined(None, &msg));
    }
}
