use crate::instance::{CoreInstance, CoreInstanceHost};

#[cfg(not(feature = "management"))]
pub(super) fn format_last_update(
    last_update: &exvlan_proto::common::RuntimeTimestamp,
) -> anyhow::Result<String> {
    // 不直接调 `Timestamp::normalized()`：该类型会随 `exvlan-proto/json-rpc`
    // feature 在 `prost_types::Timestamp` 与 `prost_wkt_types::Timestamp` 间切换，
    // 后者没有 `normalized()`。这里手动归一化 nanos，两种类型都适用。
    let mut seconds = last_update.seconds;
    let mut nanos = last_update.nanos;
    if !(0..1_000_000_000).contains(&nanos) {
        seconds += (nanos / 1_000_000_000) as i64;
        nanos %= 1_000_000_000;
        if nanos < 0 {
            nanos += 1_000_000_000;
            seconds -= 1;
        }
    }
    let date_time = chrono::DateTime::from_timestamp(seconds, nanos as u32)
        .ok_or_else(|| anyhow::anyhow!("invalid protobuf timestamp"))?;
    Ok(format!("\"{date_time:?}\""))
}

#[cfg(feature = "management")]
pub(super) fn format_last_update(
    last_update: &exvlan_proto::common::RuntimeTimestamp,
) -> anyhow::Result<String> {
    serde_json::to_string(last_update).map_err(anyhow::Error::from)
}

#[cfg(not(feature = "management"))]
pub(super) fn node_config<H>(_instance: &CoreInstance<H>) -> anyhow::Result<String>
where
    H: CoreInstanceHost,
{
    Ok(String::new())
}

#[cfg(feature = "management")]
pub(super) fn node_config<H>(instance: &CoreInstance<H>) -> anyhow::Result<String>
where
    H: CoreInstanceHost,
{
    use crate::config::toml::ConfigLoader as _;

    instance
        .toml_config()
        .ok_or_else(|| anyhow::anyhow!("shared TOML configuration is not available"))
        .map(|config| config.dump())
}
