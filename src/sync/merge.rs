//! 拉取合并：按 id 分组所有事件，取 updated_at 最大的全量快照
//! delete 事件胜出时本地物理删除；upsert 事件胜出时落库

use std::collections::HashMap;

use crate::sync::event::SyncEvent;

/// 合并结果：按 id 分组后的最终状态
pub struct MergedEvent {
    #[allow(dead_code)]
    pub id: String,
    pub event: SyncEvent,
}

/// 合并事件流：同一 id 取 updated_at 最大的事件
/// 同毫秒冲突时，delete 优先于 upsert（保证删除语义不丢）
pub fn merge_events(events: Vec<SyncEvent>) -> Vec<MergedEvent> {
    let mut latest: HashMap<String, SyncEvent> = HashMap::new();
    for ev in events {
        let id = ev.id().to_string();
        match latest.get(&id) {
            None => {
                latest.insert(id, ev);
            }
            Some(cur) => {
                let should_replace = ev.updated_at() > cur.updated_at()
                    || (ev.updated_at() == cur.updated_at() && is_delete(&ev) && !is_delete(cur));
                if should_replace {
                    latest.insert(id, ev);
                }
            }
        }
    }
    latest
        .into_iter()
        .map(|(id, event)| MergedEvent { id, event })
        .collect()
}

fn is_delete(ev: &SyncEvent) -> bool {
    matches!(ev, SyncEvent::Delete(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::event::{SyncDelete, SyncItem};

    fn upsert(id: &str, ts: i64) -> SyncEvent {
        SyncEvent::Upsert(SyncItem {
            id: id.to_string(),
            source_device: "pc".to_string(),
            kind: "text".to_string(),
            preview: "p".to_string(),
            text_content: Some("t".to_string()),
            plain_text: Some("t".to_string()),
            blob_path: None,
            note: None,
            created_at: ts,
            updated_at: ts,
        })
    }

    fn delete(id: &str, ts: i64) -> SyncEvent {
        SyncEvent::Delete(SyncDelete {
            id: id.to_string(),
            source_device: "pc".to_string(),
            updated_at: ts,
        })
    }

    #[test]
    fn newer_upsert_wins() {
        let events = vec![upsert("a", 1000), upsert("a", 2000)];
        let merged = merge_events(events);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].event.updated_at(), 2000);
    }

    #[test]
    fn delete_wins_when_newer() {
        let events = vec![upsert("a", 1000), delete("a", 2000)];
        let merged = merge_events(events);
        assert_eq!(merged.len(), 1);
        assert!(is_delete(&merged[0].event));
    }

    #[test]
    fn upsert_wins_when_newer_than_delete() {
        let events = vec![delete("a", 1000), upsert("a", 2000)];
        let merged = merge_events(events);
        assert_eq!(merged.len(), 1);
        assert!(!is_delete(&merged[0].event));
    }

    #[test]
    fn same_timestamp_delete_wins() {
        let events = vec![upsert("a", 1000), delete("a", 1000)];
        let merged = merge_events(events);
        assert!(is_delete(&merged[0].event));
    }

    #[test]
    fn different_ids_kept() {
        let events = vec![upsert("a", 1000), upsert("b", 2000)];
        let merged = merge_events(events);
        assert_eq!(merged.len(), 2);
    }
}
