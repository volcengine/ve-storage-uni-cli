//! 用户元数据参数解析；容量由服务端 TOS 校验，不在 CLI 复制预算。
use serde::de::{Deserializer, MapAccess, Visitor};
use std::collections::HashMap;

// 保留 JSON 的每一个键，避免反序列化到 map 时静默吞掉重复项。
struct MetadataEntries;

impl<'de> Visitor<'de> for MetadataEntries {
    type Value = Vec<(String, String)>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a JSON object with string values")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry::<String, String>()? {
            entries.push(entry);
        }
        Ok(entries)
    }
}

pub fn parse_user_metadata(input: &str) -> Result<HashMap<String, String>, String> {
    let entries: Vec<(String, String)> = if input.trim_start().starts_with('{') {
        let mut decoder = serde_json::Deserializer::from_str(input);
        let entries = (&mut decoder)
            .deserialize_map(MetadataEntries)
            .map_err(|_| "--meta must be a JSON object with string values".to_string())?;
        decoder
            .end()
            .map_err(|_| "unexpected trailing content in --meta".to_string())?;
        entries
    } else {
        input
            .split('#')
            .map(|pair| {
                pair.split_once('=')
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .ok_or_else(|| {
                        "--meta must be key=value pairs separated by '#' or a JSON object"
                            .to_string()
                    })
            })
            .collect::<Result<_, _>>()?
    };
    if entries.is_empty() {
        return Err("--meta must contain at least one key".into());
    }
    let mut result = HashMap::new();
    for (key, value) in entries {
        if key.is_empty()
            || !key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(
                "metadata keys must contain only ASCII letters, digits, hyphens and underscores"
                    .into(),
            );
        }
        let key = key.to_ascii_lowercase();
        if key.starts_with("ids-") {
            return Err("metadata keys starting with ids- are reserved".into());
        }
        if result.insert(key, value).is_some() {
            return Err("duplicate metadata keys after lowercase normalization".into());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_preserves_values_and_normalizes_keys_without_a_client_size_limit() {
        let meta = parse_user_metadata("Owner=xsh#empty=#expr=a=b").unwrap();
        assert_eq!(meta["owner"], "xsh");
        assert_eq!(meta["empty"], "");
        assert_eq!(meta["expr"], "a=b");
        let meta = parse_user_metadata(r#"{"Note":"中文 + %20 # = \n"}"#).unwrap();
        assert_eq!(meta["note"], "中文 + %20 # = \n");
        assert!(parse_user_metadata(&format!("note={}", "a".repeat(8193))).is_ok());
    }

    #[test]
    fn metadata_rejects_invalid_keys_values_and_case_collisions() {
        for input in [
            "",
            "missing",
            "=v",
            "a.b=v",
            " a=v",
            "ids-name=v",
            "IDS-name=v",
            "A=1#a=2",
            "a=1#a=2",
            "{}",
            r#"{"A":"1","a":"2"}"#,
            r#"{"a":"1","a":"2"}"#,
            r#"{"a":"1"} trailing"#,
            r#"{"a":1}"#,
            r#"{"a":null}"#,
        ] {
            assert!(parse_user_metadata(input).is_err(), "{input}");
        }
    }
}
