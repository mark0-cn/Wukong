//! Bounded byte-oriented string extraction.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StringsQuery {
    pub min_length: usize,
    pub max_length: usize,
    pub limit: usize,
    pub offset: u64,
    pub contains: Option<String>,
}
impl Default for StringsQuery {
    fn default() -> Self {
        Self {
            min_length: 4,
            max_length: 4096,
            limit: 50,
            offset: 0,
            contains: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryString {
    pub address: AddressRef,
    pub text: String,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringsResult {
    pub sha256: String,
    pub strings: Vec<BinaryString>,
    pub next_offset: Option<u64>,
    pub encoding: String,
}
pub fn strings(path: impl AsRef<Path>, query: &StringsQuery) -> Result<StringsResult> {
    if query.min_length == 0
        || query.min_length > query.max_length
        || query.max_length > 65536
        || query.limit == 0
        || query.limit > 5000
    {
        bail!("invalid strings bounds (length 1..65536, limit 1..5000)");
    }
    let data = read_binary(path.as_ref())?;
    let hash = sha256_bytes(&data);
    let mut pos = usize::try_from(query.offset).context("offset out of range")?;
    if pos > data.len() {
        bail!("strings offset beyond end of file");
    }
    // Resume only at a run boundary; a caller-supplied offset in a run skips it.
    if pos > 0
        && data
            .get(pos.wrapping_sub(1))
            .is_some_and(|b| (0x20..=0x7e).contains(b))
    {
        while pos < data.len() && (0x20..=0x7e).contains(&data[pos]) {
            pos += 1;
        }
    }
    let mut found = Vec::new();
    while pos < data.len() {
        if !(0x20..=0x7e).contains(&data[pos]) {
            pos += 1;
            continue;
        }
        let start = pos;
        while pos < data.len() && (0x20..=0x7e).contains(&data[pos]) {
            pos += 1;
        }
        if pos - start < query.min_length {
            continue;
        }
        let bytes = &data[start..pos];
        if query.contains.as_ref().is_some_and(|needle| {
            !bytes
                .windows(needle.len().max(1))
                .any(|w| w == needle.as_bytes())
                && !needle.is_empty()
        }) {
            continue;
        }
        let end = pos.min(start.saturating_add(query.max_length));
        found.push(BinaryString {
            address: AddressRef::file_offset(&hash, start as u64),
            text: String::from_utf8_lossy(&data[start..end]).into_owned(),
            truncated: end < pos,
        });
        if found.len() == query.limit {
            break;
        }
    }
    Ok(StringsResult {
        sha256: hash,
        strings: found,
        next_offset: (pos < data.len()).then_some(pos as u64),
        encoding: "ASCII printable 0x20..0x7e".into(),
    })
}
