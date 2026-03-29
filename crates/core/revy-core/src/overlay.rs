use std::collections::BTreeMap;

pub fn apply_optional_entry<K, V>(map: &mut BTreeMap<K, V>, key: K, value: Option<V>)
where
    K: Ord,
{
    if let Some(value) = value {
        map.insert(key, value);
    } else {
        map.remove(&key);
    }
}

pub fn apply_optional_entries<K, V, I>(map: &mut BTreeMap<K, V>, entries: I)
where
    K: Ord,
    I: IntoIterator<Item = (K, Option<V>)>,
{
    for (key, value) in entries {
        apply_optional_entry(map, key, value);
    }
}
