//! 迁移报告：每一步都要能区分「已完成 / 跳过（原因）/ 部分完成 / 失败」。
//!
//! 契约（与 GUI、`--json` 消费方）：
//! - 顶层保留 `ok` / `dry_run` / `backup` / `converted_networks` / `removed` / `ts`；
//! - 新增 `steps` 数组，`status` ∈ `planned|ok|skipped|partial|failed`，`reason` 说明为什么；
//! - `completed` / `skipped` 是 `steps` 的派生根，方便人直接读。

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// dry-run：本可以这么做，但没落盘
    Planned,
    Ok,
    Skipped,
    /// 部分成功（逐条 best-effort 里有失败，但整体不算失败）
    Partial,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Planned => "planned",
            Status::Ok => "ok",
            Status::Skipped => "skipped",
            Status::Partial => "partial",
            Status::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Step {
    pub id: &'static str,
    pub status: Status,
    pub reason: Option<String>,
    pub detail: Value,
}

impl Step {
    pub fn new(id: &'static str, status: Status) -> Self {
        Self {
            id,
            status,
            reason: None,
            detail: Value::Null,
        }
    }
    pub fn ok(id: &'static str) -> Self {
        Self::new(id, Status::Ok)
    }
    pub fn planned(id: &'static str) -> Self {
        Self::new(id, Status::Planned)
    }
    pub fn skipped(id: &'static str, reason: impl Into<String>) -> Self {
        Self::new(id, Status::Skipped).with_reason(reason)
    }
    pub fn failed(id: &'static str, reason: impl Into<String>) -> Self {
        Self::new(id, Status::Failed).with_reason(reason)
    }
    pub fn partial(id: &'static str, reason: impl Into<String>) -> Self {
        Self::new(id, Status::Partial).with_reason(reason)
    }
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = detail;
        self
    }
    pub fn is_failure(&self) -> bool {
        matches!(self.status, Status::Failed)
    }
    fn to_json(&self) -> Value {
        let mut out = json!({"id": self.id, "status": self.status.as_str()});
        if let Some(reason) = &self.reason {
            out["reason"] = json!(reason);
        }
        if !self.detail.is_null() {
            out["detail"] = self.detail.clone();
        }
        out
    }
}

/// 汇总成 `{"steps": [...], "completed": [...], "skipped": [...]}`。
pub fn steps_json(steps: &[Step]) -> Value {
    let completed: Vec<Value> = steps
        .iter()
        .filter(|s| matches!(s.status, Status::Ok | Status::Partial | Status::Planned))
        .map(|s| json!({"id": s.id, "status": s.status.as_str()}))
        .collect();
    let skipped: Vec<Value> = steps
        .iter()
        .filter(|s| matches!(s.status, Status::Skipped | Status::Failed))
        .map(|s| {
            json!({
                "id": s.id,
                "status": s.status.as_str(),
                "reason": s.reason.clone().unwrap_or_default(),
            })
        })
        .collect();
    json!({
        "steps": steps.iter().map(Step::to_json).collect::<Vec<_>>(),
        "completed": completed,
        "skipped": skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_done_from_skipped_with_reason() {
        let steps = vec![
            Step::ok("prefs").with_detail(json!({"imported": ["lang"]})),
            Step::skipped("machine_id", "新路径已存在"),
            Step::planned("stale_paths"),
        ];
        let summary = steps_json(&steps);
        assert_eq!(summary["steps"][0]["status"], "ok");
        assert_eq!(summary["steps"][1]["status"], "skipped");
        assert_eq!(summary["steps"][1]["reason"], "新路径已存在");
        assert_eq!(summary["steps"][2]["status"], "planned");
        // dry-run 的计划项算「已完成计划」，但绝不能混进 skipped
        assert_eq!(summary["completed"].as_array().unwrap().len(), 2);
        assert_eq!(summary["skipped"].as_array().unwrap().len(), 1);
    }
}
