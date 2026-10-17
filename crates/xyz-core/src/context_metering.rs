//! Source-labelled presentation accounting, independent of trust and protection.
//!
//! Only host-created locations select categories. Request structure determines the
//! estimate before labels are applied: splitting labels never creates a message,
//! a template allowance, or another rounded token estimate.
use super::{estimate_tokens, ContextError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const CATEGORY_SCHEMA_VERSION: u32 = 2;
pub const PROTOCOL_METHOD: &str = "protocol_structure_v1";
pub const MAX_SOURCE_ENTRIES: usize = 4096;
pub const MAX_PROTOCOL_SPANS: usize = 4096;
pub const MAX_LAYOUT_ROOTS: usize = 8;
pub const MAX_SOURCE_MAP_BYTES: usize = 1024 * 1024;
// BTreeSet allocation layout is private to std. Charge a deliberately generous
// node/edge allowance per occupied root, in addition to each String capacity.
const BTREE_ROOT_ALLOWANCE_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayCategory {
    SystemInstructions,
    ToolDefinitions,
    FixedRules,
    HistorySummary,
    CurrentConversation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialoguePart {
    UserMessages,
    AssistantMessages,
    ToolCalls,
    ToolResults,
    OtherInputs,
}
const CATEGORIES: [DisplayCategory; 5] = [
    DisplayCategory::SystemInstructions,
    DisplayCategory::ToolDefinitions,
    DisplayCategory::FixedRules,
    DisplayCategory::HistorySummary,
    DisplayCategory::CurrentConversation,
];
const DIALOGUE_PARTS: [DialoguePart; 5] = [
    DialoguePart::UserMessages,
    DialoguePart::AssistantMessages,
    DialoguePart::ToolCalls,
    DialoguePart::ToolResults,
    DialoguePart::OtherInputs,
];

/// Half-open UTF-8 byte range in a decoded string (not its JSON encoding).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLocation {
    /// RFC 6901 pointer in the final transport body.
    pub pointer: String,
    /// RFC 6901 pointer inside an explicitly declared embedded JSON string.
    pub embedded_pointer: Option<String>,
    /// None selects the whole value, including descendants of an object/array.
    pub span: Option<SourceSpan>,
}
impl SourceLocation {
    pub fn new(pointer: impl Into<String>) -> Self {
        Self {
            pointer: pointer.into(),
            embedded_pointer: None,
            span: None,
        }
    }
    pub fn embedded(pointer: impl Into<String>, embedded_pointer: impl Into<String>) -> Self {
        Self {
            pointer: pointer.into(),
            embedded_pointer: Some(embedded_pointer.into()),
            span: None,
        }
    }
    pub fn with_span(mut self, start: usize, end: usize) -> Self {
        self.span = Some(SourceSpan { start, end });
        self
    }
}
#[derive(Clone, Debug)]
pub struct SourceEntry {
    pub source_id: String,
    pub location: SourceLocation,
    pub category: DisplayCategory,
    pub dialogue_part: Option<DialoguePart>,
}
impl SourceEntry {
    pub fn new(
        source_id: impl Into<String>,
        location: SourceLocation,
        category: DisplayCategory,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            location,
            category,
            dialogue_part: None,
        }
    }
    pub fn dialogue(
        source_id: impl Into<String>,
        location: SourceLocation,
        part: DialoguePart,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            location,
            category: DisplayCategory::CurrentConversation,
            dialogue_part: Some(part),
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct RequestSourceMap {
    pub entries: Vec<SourceEntry>,
    /// Host-declared layout, independent of category entries. User JSON is not
    /// discovered by inspecting its keys, prefixes, or apparent instructions.
    pub embedded_json: BTreeSet<String>,
    /// Host-inserted explanatory wrappers. These are framing, never a sixth
    /// category. A full content entry may cover them; protocol takes precedence.
    pub protocol_spans: Vec<SourceLocation>,
    classification_invalid: bool,
    layout_invalid: bool,
}
impl RequestSourceMap {
    pub fn add(&mut self, entry: SourceEntry) {
        if self.classification_invalid {
            return;
        }
        let growth = if self.entries.len() == self.entries.capacity() {
            std::mem::size_of::<SourceEntry>()
        } else {
            0
        };
        if self.entries.len() >= MAX_SOURCE_ENTRIES
            || self
                .estimated_memory_bytes()
                .saturating_add(growth)
                .saturating_add(entry_heap_bytes(&entry))
                > MAX_SOURCE_MAP_BYTES
        {
            self.classification_invalid = true;
            return;
        }
        // Bound capacity growth as well as len; push's geometric allocation can
        // otherwise exceed a metadata reservation before its next validation.
        self.entries.reserve_exact(1);
        self.entries.push(entry);
    }
    pub fn add_protocol(&mut self, location: SourceLocation) {
        if self.layout_invalid {
            return;
        }
        let growth = if self.protocol_spans.len() == self.protocol_spans.capacity() {
            std::mem::size_of::<SourceLocation>()
        } else {
            0
        };
        if self.protocol_spans.len() >= MAX_PROTOCOL_SPANS
            || self
                .estimated_memory_bytes()
                .saturating_add(growth)
                .saturating_add(location_heap_bytes(&location))
                > MAX_SOURCE_MAP_BYTES
        {
            // A dropped wrapper cannot quietly become ordinary content. The
            // independent accounting layout is incomplete and sending must fail.
            self.layout_invalid = true;
            return;
        }
        self.protocol_spans.reserve_exact(1);
        self.protocol_spans.push(location);
    }
    pub fn add_embedded_json(&mut self, pointer: impl Into<String>) {
        let pointer = pointer.into();
        if self.layout_invalid || self.embedded_json.contains(&pointer) {
            return;
        }
        if self.embedded_json.len() >= MAX_LAYOUT_ROOTS
            || self
                .estimated_memory_bytes()
                .saturating_add(BTREE_ROOT_ALLOWANCE_BYTES)
                .saturating_add(pointer.capacity())
                > MAX_SOURCE_MAP_BYTES
        {
            self.layout_invalid = true;
            return;
        }
        self.embedded_json.insert(pointer);
    }
    /// Conservative managed metadata estimate, including spare vector slots and
    /// string capacity. This is an allocation estimate, not an RSS measurement.
    pub fn estimated_memory_bytes(&self) -> usize {
        self.entries.iter().fold(
            self.layout_memory_bytes().saturating_add(
                self.entries
                    .capacity()
                    .saturating_mul(std::mem::size_of::<SourceEntry>()),
            ),
            |bytes, entry| bytes.saturating_add(entry_heap_bytes(entry)),
        )
    }
    fn layout_memory_bytes(&self) -> usize {
        let bytes = self.protocol_spans.iter().fold(
            std::mem::size_of::<Self>().saturating_add(
                self.protocol_spans
                    .capacity()
                    .saturating_mul(std::mem::size_of::<SourceLocation>()),
            ),
            |bytes, location| bytes.saturating_add(location_heap_bytes(location)),
        );
        self.embedded_json.iter().fold(bytes, |bytes, pointer| {
            bytes
                .saturating_add(BTREE_ROOT_ALLOWANCE_BYTES)
                .saturating_add(pointer.capacity())
        })
    }
}
fn location_heap_bytes(location: &SourceLocation) -> usize {
    location.pointer.capacity().saturating_add(
        location
            .embedded_pointer
            .as_ref()
            .map_or(0, String::capacity),
    )
}
fn entry_heap_bytes(entry: &SourceEntry) -> usize {
    entry
        .source_id
        .capacity()
        .saturating_add(location_heap_bytes(&entry.location))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DialogueEstimate {
    pub category: DialoguePart,
    pub estimated_tokens: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentEstimate {
    pub category: DisplayCategory,
    pub estimated_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<DialogueEstimate>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProtocolOverhead {
    pub method: String,
    pub known_framing_tokens: Option<u64>,
    pub template_allowance_tokens: Option<u64>,
    pub estimated_tokens: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct RequestMetering {
    pub categories: Vec<ContentEstimate>,
    pub content_input_tokens: u64,
    pub protocol_overhead: ProtocolOverhead,
    pub input_tokens: u64,
    /// Static diagnostic code only: neither source text nor credentials.
    pub classification_error: Option<String>,
}

#[derive(Debug)]
struct Unit {
    pointer: String,
    embedded_pointer: Option<String>,
    text: String,
    /// JSON containers (tools/calls) are canonical content units. Ranges are
    /// valid only on decoded string units, not container encodings.
    string: bool,
}
#[derive(Default)]
struct Structure {
    units: Vec<Unit>,
    framing: String,
    message_count: u64,
    tool_count: u64,
    call_count: u64,
    embedded_seen: BTreeSet<String>,
}
fn total_error() -> ContextError {
    ContextError::new(
        "CONTEXT_METERING_TOTAL_UNAVAILABLE",
        "Unsupported or inconsistent request structure; request must not be sent.",
    )
}
fn child_pointer(parent: &str, key: &str) -> String {
    format!("{}/{}", parent, key.replace('~', "~0").replace('/', "~1"))
}
fn descendant(path: &str, parent: &str) -> bool {
    path == parent || (path.starts_with(parent) && path.as_bytes().get(parent.len()) == Some(&b'/'))
}
fn valid_pointer(pointer: &str) -> bool {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return false;
    }
    let mut chars = pointer.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}
impl Structure {
    fn atomic(&mut self, pointer: String, value: &Value) {
        self.units.push(Unit {
            pointer,
            embedded_pointer: None,
            text: value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
            string: value.is_string(),
        });
    }
    fn content(
        &mut self,
        pointer: String,
        value: &Value,
        map: &RequestSourceMap,
    ) -> Result<(), ContextError> {
        if map.embedded_json.contains(&pointer) {
            let text = value.as_str().ok_or_else(total_error)?;
            let parsed: Value = serde_json::from_str(text).map_err(|_| total_error())?;
            // Exact canonical serialization prevents location drift and silently
            // discarding noncanonical whitespace or differently encoded escapes.
            let canonical = serde_json::to_string(&parsed).map_err(|_| total_error())?;
            if canonical.as_str() != text {
                return Err(total_error());
            }
            self.embedded_seen.insert(pointer.clone());
            self.embedded(&pointer, "", &parsed);
        } else if !value.is_null() {
            self.atomic(pointer, value);
        }
        Ok(())
    }
    fn embedded(&mut self, outer: &str, inner: &str, value: &Value) {
        match value {
            Value::Object(object) => {
                self.framing.push('{');
                for (index, (key, child)) in object.iter().enumerate() {
                    if index > 0 {
                        self.framing.push(',');
                    }
                    self.framing
                        .push_str(&Value::String(key.clone()).to_string());
                    self.framing.push(':');
                    self.embedded(outer, &child_pointer(inner, key), child);
                }
                self.framing.push('}');
            }
            Value::Array(array) => {
                self.framing.push('[');
                for (index, child) in array.iter().enumerate() {
                    if index > 0 {
                        self.framing.push(',');
                    }
                    self.embedded(outer, &child_pointer(inner, &index.to_string()), child);
                }
                self.framing.push(']');
            }
            _ => {
                if let Some(text) = value.as_str() {
                    // Quotes and escape expansion exist in the model's embedded
                    // JSON input. Outer HTTP escaping is not counted a second time.
                    self.framing
                        .push_str(&" ".repeat(value.to_string().len() - text.len()));
                }
                self.units.push(Unit {
                    pointer: outer.into(),
                    embedded_pointer: Some(inner.into()),
                    text: value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                    string: value.is_string(),
                });
            }
        }
    }
    fn text_parts(
        &mut self,
        pointer: &str,
        value: &Value,
        map: &RequestSourceMap,
    ) -> Result<(), ContextError> {
        self.framing.push('[');
        for (index, part) in value.as_array().ok_or_else(total_error)?.iter().enumerate() {
            let part = part.as_object().ok_or_else(total_error)?;
            if part.len() != 2
                || !part
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| matches!(kind, "text" | "input_text" | "output_text"))
                || !part.get("text").is_some_and(Value::is_string)
            {
                return Err(total_error());
            }
            if index > 0 {
                self.framing.push(',');
            }
            self.framing.push_str("{\"type\":");
            self.framing.push_str(&part["type"].to_string());
            self.framing.push_str(",\"text\":\"\"}");
            self.content(
                child_pointer(&child_pointer(pointer, &index.to_string()), "text"),
                &part["text"],
                map,
            )?;
        }
        self.framing.push(']');
        Ok(())
    }
    fn call(
        &mut self,
        pointer: &str,
        value: &Value,
        map: &RequestSourceMap,
    ) -> Result<(), ContextError> {
        let object = value.as_object().ok_or_else(total_error)?;
        self.call_count += 1;
        self.framing.push('{');
        for (index, (key, value)) in object.iter().enumerate() {
            if index > 0 {
                self.framing.push(',');
            }
            self.framing
                .push_str(&Value::String(key.clone()).to_string());
            self.framing.push(':');
            match key.as_str() {
                "id" | "type" | "index" => self.framing.push_str(&value.to_string()),
                "function" => {
                    // An OpenAI call's function envelope contains name/arguments.
                    self.call_count -= 1;
                    self.call(&child_pointer(pointer, key), value, map)?;
                }
                "name" | "arguments" => {
                    if !value.is_string() {
                        return Err(total_error());
                    }
                    self.framing.push_str("\"\"");
                    self.content(child_pointer(pointer, key), value, map)?;
                }
                _ => return Err(total_error()),
            }
        }
        self.framing.push('}');
        Ok(())
    }
    fn messages(
        &mut self,
        pointer: &str,
        messages: &Value,
        map: &RequestSourceMap,
    ) -> Result<(), ContextError> {
        let messages = messages.as_array().ok_or_else(total_error)?;
        self.framing.push('[');
        for (index, message) in messages.iter().enumerate() {
            let message = message.as_object().ok_or_else(total_error)?;
            if !message
                .get("role")
                .and_then(Value::as_str)
                .is_some_and(|role| {
                    matches!(
                        role,
                        "system" | "developer" | "user" | "assistant" | "tool" | "function"
                    )
                })
            {
                return Err(total_error());
            }
            self.message_count += 1;
            if index > 0 {
                self.framing.push(',');
            }
            self.framing.push('{');
            for (field_index, (key, value)) in message.iter().enumerate() {
                if field_index > 0 {
                    self.framing.push(',');
                }
                self.framing
                    .push_str(&Value::String(key.clone()).to_string());
                self.framing.push(':');
                let path = child_pointer(&child_pointer(pointer, &index.to_string()), key);
                match key.as_str() {
                    "role" | "name" | "tool_call_id" | "id" => {
                        if !value.is_string() {
                            return Err(total_error());
                        }
                        self.framing.push_str(&value.to_string());
                    }
                    "content" => {
                        // No hidden image/audio token cost can be safely inferred
                        // from an URL. This version accepts textual requests only.
                        if value.is_array() {
                            self.text_parts(&path, value, map)?;
                        } else if value.is_string() {
                            self.framing.push_str("\"\"");
                            self.content(path, value, map)?;
                        } else if value.is_null() {
                            self.framing.push_str("null");
                        } else {
                            return Err(total_error());
                        }
                    }
                    "tool_calls" => {
                        let calls = value.as_array().ok_or_else(total_error)?;
                        self.framing.push('[');
                        for (call_index, call) in calls.iter().enumerate() {
                            if call_index > 0 {
                                self.framing.push(',');
                            }
                            self.call(&child_pointer(&path, &call_index.to_string()), call, map)?;
                        }
                        self.framing.push(']');
                    }
                    "function_call" => self.call(&path, value, map)?,
                    // Unknown input properties could carry a modality or token
                    // source whose independent total is not reliable.
                    _ => return Err(total_error()),
                }
            }
            self.framing.push('}');
        }
        self.framing.push(']');
        Ok(())
    }
}

/// Estimate a supported final text request. Invalid source labels disable only
/// classification; invalid request/layout structure fails closed for sending.
pub fn meter_request(
    body: &Value,
    sources: &RequestSourceMap,
) -> Result<RequestMetering, ContextError> {
    // Public vectors permit host construction, so validate even maps that did
    // not use the bounded add methods before resolving or allocating ranges.
    if sources.layout_invalid
        || sources.protocol_spans.len() > MAX_PROTOCOL_SPANS
        || sources.embedded_json.len() > MAX_LAYOUT_ROOTS
        || sources.layout_memory_bytes() > MAX_SOURCE_MAP_BYTES
    {
        return Err(total_error());
    }
    let classification_limited = sources.classification_invalid
        || sources.entries.len() > MAX_SOURCE_ENTRIES
        || sources.estimated_memory_bytes() > MAX_SOURCE_MAP_BYTES;
    let object = body.as_object().ok_or_else(total_error)?;
    if !object.contains_key("messages") && !object.contains_key("input") {
        return Err(total_error());
    }
    if object.contains_key("messages") && object.contains_key("input") {
        return Err(total_error());
    }
    let mut structure = Structure::default();
    structure.framing.push('{');
    let mut input_fields = 0;
    for (key, value) in object {
        let semantic = matches!(
            key.as_str(),
            "messages" | "input" | "system" | "instructions" | "tools" | "functions"
        );
        if !semantic {
            // These fields configure the transport/model, not model input.
            // Explicit allowlisting makes unknown future input shapes fail closed.
            if !matches!(
                key.as_str(),
                "model"
                    | "max_tokens"
                    | "max_completion_tokens"
                    | "max_output_tokens"
                    | "temperature"
                    | "top_p"
                    | "n"
                    | "stream"
                    | "stream_options"
                    | "stop"
                    | "presence_penalty"
                    | "frequency_penalty"
                    | "logit_bias"
                    | "logprobs"
                    | "top_logprobs"
                    | "seed"
                    | "user"
                    | "store"
                    | "metadata"
                    | "service_tier"
                    | "reasoning_effort"
                    | "parallel_tool_calls"
                    | "tool_choice"
                    | "function_call"
            ) {
                return Err(total_error());
            }
            continue;
        }
        if input_fields > 0 {
            structure.framing.push(',');
        }
        input_fields += 1;
        structure
            .framing
            .push_str(&Value::String(key.clone()).to_string());
        structure.framing.push(':');
        let pointer = child_pointer("", key);
        match key.as_str() {
            "messages" => structure.messages(&pointer, value, sources)?,
            "input" if value.is_array() => structure.messages(&pointer, value, sources)?,
            "input" | "system" | "instructions" => {
                if !value.is_string() {
                    return Err(total_error());
                }
                structure.framing.push_str("\"\"");
                structure.content(pointer, value, sources)?;
            }
            "tools" | "functions" => {
                let tools = value.as_array().ok_or_else(total_error)?;
                structure.tool_count += tools.len() as u64;
                structure.framing.push('[');
                for (index, tool) in tools.iter().enumerate() {
                    if !tool.is_object() {
                        return Err(total_error());
                    }
                    if index > 0 {
                        structure.framing.push(',');
                    }
                    structure.atomic(child_pointer(&pointer, &index.to_string()), tool);
                }
                structure.framing.push(']');
            }
            _ => unreachable!(),
        }
    }
    structure.framing.push('}');
    if structure.embedded_seen != sources.embedded_json {
        return Err(total_error());
    }
    // Temporary attribution storage grows with host source ranges, not with
    // bytes or characters in the request. Text is scanned only after sorting.
    let mut protocol: Vec<Vec<Interval>> = structure.units.iter().map(|_| Vec::new()).collect();
    for location in &sources.protocol_spans {
        let ranges =
            resolve_location(body, &structure.units, location).map_err(|_| total_error())?;
        for (index, start, end) in ranges {
            if start < end {
                protocol[index].push((start, end));
            }
        }
    }
    for ranges in &mut protocol {
        ranges.sort_unstable();
        if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(total_error());
        }
    }
    let owners = if classification_limited {
        Err("source_map_limit_exceeded")
    } else {
        source_ranges(body, sources, &structure.units, &protocol)
    };
    let (owners, mut classification_error) = match owners {
        Ok(owners) => (owners, None),
        Err(code) => (Vec::new(), Some(code)),
    };
    let mut known_framing = estimate_tokens(&structure.framing);
    let mut content = 0;
    let mut category_tokens = BTreeMap::<DisplayCategory, u64>::new();
    let mut child_tokens = BTreeMap::<DialoguePart, u64>::new();
    for (index, unit) in structure.units.iter().enumerate() {
        let mut ascii_run = 0u64;
        let mut protocol_index = 0;
        let mut owner_index = 0;
        for (start, character) in unit.text.char_indices() {
            let tokens = if character.is_ascii() {
                ascii_run += 1;
                u64::from(ascii_run % 2 == 1)
            } else {
                ascii_run = 0;
                character.len_utf8() as u64
            };
            while protocol[index]
                .get(protocol_index)
                .is_some_and(|range| range.1 <= start)
            {
                protocol_index += 1;
            }
            if protocol[index]
                .get(protocol_index)
                .is_some_and(|range| range.0 <= start)
            {
                known_framing += tokens;
            } else {
                content += tokens;
                if classification_error.is_none() {
                    let ranges = &owners[index];
                    while ranges
                        .get(owner_index)
                        .is_some_and(|range| range.end <= start)
                    {
                        owner_index += 1;
                    }
                    if let Some(range) =
                        ranges.get(owner_index).filter(|range| range.start <= start)
                    {
                        let entry = &sources.entries[range.owner];
                        *category_tokens.entry(entry.category).or_default() += tokens;
                        if let Some(part) = entry.dialogue_part {
                            *child_tokens.entry(part).or_default() += tokens;
                        }
                    } else {
                        // Keep scanning to retain the independent hard-budget
                        // total, but discard all partial display attribution.
                        classification_error = Some("missing_source_entries");
                    }
                }
            }
        }
    }
    // Versioned engineering reserve for unknown provider templates; it is not a
    // provider-reported token count. It depends only on actual request structure.
    let allowance =
        16 + 8 * structure.message_count + 8 * structure.tool_count + 8 * structure.call_count;
    let overhead = known_framing + allowance;
    let categories = if classification_error.is_none() {
        counted_categories(&category_tokens, &child_tokens)
    } else {
        unavailable_categories()
    };
    Ok(RequestMetering {
        categories,
        content_input_tokens: content,
        protocol_overhead: ProtocolOverhead {
            method: PROTOCOL_METHOD.into(),
            known_framing_tokens: Some(known_framing),
            template_allowance_tokens: Some(allowance),
            estimated_tokens: Some(overhead),
        },
        input_tokens: content + overhead,
        classification_error: classification_error.map(str::to_owned),
    })
}

type ResolvedRange = (usize, usize, usize);
fn resolve_location(
    body: &Value,
    units: &[Unit],
    location: &SourceLocation,
) -> Result<Vec<ResolvedRange>, &'static str> {
    if !valid_pointer(&location.pointer)
        || location
            .embedded_pointer
            .as_ref()
            .is_some_and(|path| !valid_pointer(path))
    {
        return Err("invalid_source_location");
    }
    let outer = body
        .pointer(&location.pointer)
        .ok_or("missing_source_location")?;
    let parsed;
    let target = if let Some(inner) = &location.embedded_pointer {
        parsed = serde_json::from_str::<Value>(outer.as_str().ok_or("invalid_embedded_source")?)
            .map_err(|_| "invalid_embedded_source")?;
        parsed.pointer(inner).ok_or("missing_embedded_source")?
    } else {
        outer
    };
    if let Some(span) = &location.span {
        let text = target.as_str().ok_or("source_span_requires_string")?;
        if span.start > span.end
            || !text.is_char_boundary(span.start)
            || !text.is_char_boundary(span.end)
        {
            return Err("invalid_source_span");
        }
    }
    let mut ranges = Vec::new();
    for (index, unit) in units.iter().enumerate() {
        let selected = match &location.embedded_pointer {
            Some(inner) => {
                unit.pointer == location.pointer
                    && unit
                        .embedded_pointer
                        .as_ref()
                        .is_some_and(|path| descendant(path, inner))
            }
            None => descendant(&unit.pointer, &location.pointer),
        };
        if !selected {
            continue;
        }
        let (start, end) = if let Some(span) = &location.span {
            if !unit.string
                || unit.pointer != location.pointer
                || unit.embedded_pointer != location.embedded_pointer
            {
                return Err("source_span_requires_exact_string");
            }
            (span.start, span.end)
        } else {
            (0, unit.text.len())
        };
        ranges.push((index, start, end));
    }
    // Empty containers and null carry structure but no content. Their presence
    // is still validated through the actual body pointer above.
    if ranges.is_empty()
        && !target.is_null()
        && !target.as_array().is_some_and(Vec::is_empty)
        && !target.as_object().is_some_and(serde_json::Map::is_empty)
    {
        return Err("unmetered_source_location");
    }
    Ok(ranges)
}

fn empty_categories(known: bool) -> Vec<ContentEstimate> {
    CATEGORIES
        .into_iter()
        .map(|category| ContentEstimate {
            category,
            estimated_tokens: known.then_some(0),
            children: (category == DisplayCategory::CurrentConversation).then(|| {
                DIALOGUE_PARTS
                    .into_iter()
                    .map(|category| DialogueEstimate {
                        category,
                        estimated_tokens: known.then_some(0),
                    })
                    .collect()
            }),
        })
        .collect()
}
/// A display-only unavailable value. It must never replace an independent total
/// estimate or relax the caller's hard token/byte admission checks.
pub fn unavailable_categories() -> Vec<ContentEstimate> {
    empty_categories(false)
}
type Interval = (usize, usize);
#[derive(Debug)]
struct OwnerRange {
    start: usize,
    end: usize,
    owner: usize,
}
fn source_ranges(
    body: &Value,
    map: &RequestSourceMap,
    units: &[Unit],
    protocol: &[Vec<Interval>],
) -> Result<Vec<Vec<OwnerRange>>, &'static str> {
    let mut owners: Vec<Vec<OwnerRange>> = units.iter().map(|_| Vec::new()).collect();
    for (entry_index, entry) in map.entries.iter().enumerate() {
        if entry.source_id.is_empty()
            || (entry.category == DisplayCategory::CurrentConversation)
                != entry.dialogue_part.is_some()
        {
            return Err("invalid_source_entry");
        }
        for (index, start, end) in resolve_location(body, units, &entry.location)? {
            // Subtract framing before detecting overlapping owners. A complete
            // source may cover a host wrapper without counting it as content.
            let mut cursor = start;
            for &(protocol_start, protocol_end) in &protocol[index] {
                if protocol_end <= cursor {
                    continue;
                }
                if protocol_start >= end {
                    break;
                }
                if cursor < protocol_start {
                    owners[index].push(OwnerRange {
                        start: cursor,
                        end: protocol_start.min(end),
                        owner: entry_index,
                    });
                }
                cursor = protocol_end.max(cursor);
                if cursor >= end {
                    break;
                }
            }
            if cursor < end {
                owners[index].push(OwnerRange {
                    start: cursor,
                    end,
                    owner: entry_index,
                });
            }
        }
    }
    for ranges in &mut owners {
        ranges.sort_unstable_by_key(|range| (range.start, range.end));
        if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return Err("overlapping_source_entries");
        }
    }
    Ok(owners)
}
fn counted_categories(
    categories: &BTreeMap<DisplayCategory, u64>,
    children: &BTreeMap<DialoguePart, u64>,
) -> Vec<ContentEstimate> {
    let mut result = empty_categories(true);
    for row in &mut result {
        row.estimated_tokens = Some(categories.get(&row.category).copied().unwrap_or(0));
        if let Some(parts) = &mut row.children {
            for part in parts {
                part.estimated_tokens = Some(children.get(&part.category).copied().unwrap_or(0));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dialogue_map(pointer: &str) -> RequestSourceMap {
        let mut map = RequestSourceMap::default();
        map.add(SourceEntry::dialogue(
            "synthetic-user",
            SourceLocation::new(pointer),
            DialoguePart::UserMessages,
        ));
        map
    }
    fn assert_reconciles(result: &RequestMetering) {
        assert!(result.classification_error.is_none());
        let sum: u64 = result
            .categories
            .iter()
            .map(|row| row.estimated_tokens.unwrap())
            .sum();
        assert_eq!(sum, result.content_input_tokens);
        assert_eq!(
            sum + result.protocol_overhead.estimated_tokens.unwrap(),
            result.input_tokens
        );
        let dialogue = result
            .categories
            .iter()
            .find(|row| row.category == DisplayCategory::CurrentConversation)
            .unwrap();
        assert_eq!(
            dialogue
                .children
                .as_ref()
                .unwrap()
                .iter()
                .map(|row| row.estimated_tokens.unwrap())
                .sum::<u64>(),
            dialogue.estimated_tokens.unwrap()
        );
    }
    #[test]
    fn five_categories_and_dialogue_children_reconcile() {
        let body = json!({"model":"synthetic","messages":[{"role":"system","content":"Role. Rule."},{"role":"user","content":"Question"},{"role":"assistant","content":"Answer"},{"role":"tool","tool_call_id":"local","content":"Fixture output"}],"tools":[{"type":"function","function":{"name":"fixture","parameters":{"type":"object"}}}]});
        let mut map = RequestSourceMap::default();
        map.add(SourceEntry::new(
            "role",
            SourceLocation::new("/messages/0/content").with_span(0, 6),
            DisplayCategory::SystemInstructions,
        ));
        map.add(SourceEntry::new(
            "rules",
            SourceLocation::new("/messages/0/content").with_span(6, 11),
            DisplayCategory::FixedRules,
        ));
        map.add(SourceEntry::dialogue(
            "u",
            SourceLocation::new("/messages/1/content"),
            DialoguePart::UserMessages,
        ));
        map.add(SourceEntry::dialogue(
            "a",
            SourceLocation::new("/messages/2/content"),
            DialoguePart::AssistantMessages,
        ));
        map.add(SourceEntry::dialogue(
            "t",
            SourceLocation::new("/messages/3/content"),
            DialoguePart::ToolResults,
        ));
        map.add(SourceEntry::new(
            "schema",
            SourceLocation::new("/tools"),
            DisplayCategory::ToolDefinitions,
        ));
        assert_reconciles(&meter_request(&body, &map).unwrap());
    }
    #[test]
    fn splitting_labels_never_changes_input_or_protocol_or_rounding() {
        let body = json!({"messages":[{"role":"user","content":"abc中文🙂defghi"}]});
        let original = meter_request(&body, &dialogue_map("/messages/0/content")).unwrap();
        let text = body["messages"][0]["content"].as_str().unwrap();
        let mut map = RequestSourceMap::default();
        for (start, c) in text.char_indices() {
            map.add(SourceEntry::dialogue(
                "u",
                SourceLocation::new("/messages/0/content").with_span(start, start + c.len_utf8()),
                DialoguePart::UserMessages,
            ));
        }
        let split = meter_request(&body, &map).unwrap();
        assert_reconciles(&split);
        assert_eq!(original.input_tokens, split.input_tokens);
        assert_eq!(original.content_input_tokens, split.content_input_tokens);
        assert_eq!(
            original.protocol_overhead.estimated_tokens,
            split.protocol_overhead.estimated_tokens
        );
        assert_eq!(original.content_input_tokens, estimate_tokens(text));
    }
    #[test]
    fn category_failure_preserves_independent_guard() {
        let body = json!({"messages":[{"role":"user","content":"synthetic"}]});
        let good = meter_request(&body, &dialogue_map("/messages/0/content")).unwrap();
        let missing = meter_request(&body, &RequestSourceMap::default()).unwrap();
        assert_eq!(good.input_tokens, missing.input_tokens);
        assert_eq!(
            good.protocol_overhead.estimated_tokens,
            missing.protocol_overhead.estimated_tokens
        );
        assert_eq!(
            missing.classification_error.as_deref(),
            Some("missing_source_entries")
        );
        assert!(missing
            .categories
            .iter()
            .all(|row| row.estimated_tokens.is_none()));
        let mut overlapping = dialogue_map("/messages/0/content");
        overlapping.add(overlapping.entries[0].clone());
        let result = meter_request(&body, &overlapping).unwrap();
        assert_eq!(
            result.classification_error.as_deref(),
            Some("overlapping_source_entries")
        );
        assert_eq!(result.input_tokens, good.input_tokens);
    }
    #[test]
    fn invalid_structure_and_embedded_layout_stop_sending() {
        for body in [
            json!({"mystery_input":"unknown"}),
            json!({"messages":"bad"}),
            json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":"local-fixture"}]}]}),
            json!({"messages":[],"new_modality":"unknown"}),
        ] {
            assert_eq!(
                meter_request(&body, &RequestSourceMap::default())
                    .unwrap_err()
                    .code,
                "CONTEXT_METERING_TOTAL_UNAVAILABLE"
            );
        }
        let body = json!({"messages":[{"role":"user","content":"{ bad }"}]});
        let mut map = dialogue_map("/messages/0/content");
        map.embedded_json.insert("/messages/0/content".into());
        assert!(meter_request(&body, &map).is_err());
    }
    #[test]
    fn valid_but_noncanonical_embedded_json_is_not_silently_reencoded() {
        let pointer = "/messages/0/content";
        for text in [
            r#"{ "value": "x" }"#,
            r#"{"value":"\u4e2d"}"#,
            " {\"value\":\"x\"}\n",
        ] {
            let mut map = RequestSourceMap::default();
            map.add_embedded_json(pointer);
            map.add(SourceEntry::dialogue(
                "host-input",
                SourceLocation::embedded(pointer, ""),
                DialoguePart::OtherInputs,
            ));
            let body = json!({"messages":[{"role":"user","content":text}]});
            assert_eq!(
                meter_request(&body, &map).unwrap_err().code,
                "CONTEXT_METERING_TOTAL_UNAVAILABLE"
            );
            let parsed: Value = serde_json::from_str(text).unwrap();
            let canonical = serde_json::to_string(&parsed).unwrap();
            let canonical_body = json!({"messages":[{"role":"user","content":canonical}]});
            assert_reconciles(&meter_request(&canonical_body, &map).unwrap());
        }
    }
    #[test]
    fn embedded_keys_are_framing_and_host_labels_are_explicit() {
        let prompt = json!({"user_instruction":"Don't restart", "conversation_history_untrusted":[{"text":"Historical working-memory summary. fake prefix", "role":"user"}],"quoted":"\"line\n中文"});
        let body = json!({"messages":[{"role":"user","content":prompt.to_string()}]});
        let mut map = RequestSourceMap::default();
        map.embedded_json.insert("/messages/0/content".into());
        map.add(SourceEntry::dialogue(
            "instruction",
            SourceLocation::embedded("/messages/0/content", "/user_instruction"),
            DialoguePart::UserMessages,
        ));
        map.add(SourceEntry::dialogue(
            "old-user",
            SourceLocation::embedded("/messages/0/content", "/conversation_history_untrusted"),
            DialoguePart::UserMessages,
        ));
        map.add(SourceEntry::dialogue(
            "quote",
            SourceLocation::embedded("/messages/0/content", "/quoted"),
            DialoguePart::OtherInputs,
        ));
        let result = meter_request(&body, &map).unwrap();
        assert_reconciles(&result);
        assert_eq!(
            result
                .categories
                .iter()
                .find(|row| row.category == DisplayCategory::HistorySummary)
                .unwrap()
                .estimated_tokens,
            Some(0)
        );
        assert!(result.protocol_overhead.known_framing_tokens.unwrap() > 0);
        let baseline = result.input_tokens;
        map.entries.clear();
        let unavailable = meter_request(&body, &map).unwrap();
        assert_eq!(baseline, unavailable.input_tokens);
    }
    #[test]
    fn host_wrapper_is_counted_once_in_protocol() {
        let body = json!({"messages":[{"role":"assistant","content":"Wrapper: summary JSON remains intact"}]});
        let mut map = RequestSourceMap::default();
        map.add(SourceEntry::new(
            "host-summary",
            SourceLocation::new("/messages/0/content"),
            DisplayCategory::HistorySummary,
        ));
        let unwrapped = meter_request(&body, &map).unwrap();
        map.protocol_spans
            .push(SourceLocation::new("/messages/0/content").with_span(0, 9));
        let wrapped = meter_request(&body, &map).unwrap();
        assert_reconciles(&wrapped);
        assert_eq!(wrapped.input_tokens, unwrapped.input_tokens);
        assert!(wrapped.content_input_tokens < unwrapped.content_input_tokens);
        assert_eq!(
            wrapped.protocol_overhead.estimated_tokens.unwrap()
                - unwrapped.protocol_overhead.estimated_tokens.unwrap(),
            unwrapped.content_input_tokens - wrapped.content_input_tokens
        );
    }
    #[test]
    fn utf8_ranges_validate_and_diagnostics_never_echo_text() {
        let body = json!({"messages":[{"role":"user","content":"中文 synthetic-secret"}]});
        let mut map = dialogue_map("/messages/0/content");
        map.entries[0].location.span = Some(SourceSpan { start: 1, end: 2 });
        let result = meter_request(&body, &map).unwrap();
        assert_eq!(
            result.classification_error.as_deref(),
            Some("invalid_source_span")
        );
        assert!(!result
            .classification_error
            .unwrap()
            .contains("synthetic-secret"));
    }
    #[test]
    fn real_duplicate_messages_count_twice_but_transport_config_does_not() {
        let one = json!({"messages":[{"role":"user","content":"same"}]});
        let two = json!({"model":"synthetic-model","max_tokens":4096,"messages":[{"role":"user","content":"same"},{"role":"user","content":"same"}]});
        let first = meter_request(&one, &dialogue_map("/messages/0/content")).unwrap();
        let mut map = dialogue_map("/messages/0/content");
        map.add(SourceEntry::dialogue(
            "u2",
            SourceLocation::new("/messages/1/content"),
            DialoguePart::UserMessages,
        ));
        let second = meter_request(&two, &map).unwrap();
        assert_reconciles(&second);
        assert_eq!(second.content_input_tokens, first.content_input_tokens * 2);
        let mut config = one.clone();
        config["model"] = json!("another-synthetic-model");
        config["max_tokens"] = json!(123);
        assert_eq!(
            first.input_tokens,
            meter_request(&config, &dialogue_map("/messages/0/content"))
                .unwrap()
                .input_tokens
        );
    }
    #[test]
    fn empty_sources_are_zero_and_protocol_is_still_present() {
        let body = json!({"messages":[]});
        let result = meter_request(&body, &RequestSourceMap::default()).unwrap();
        assert_reconciles(&result);
        assert_eq!(result.content_input_tokens, 0);
        assert!(result.input_tokens > 0);
    }
    #[test]
    fn text_parts_and_tool_call_payloads_reconcile() {
        let body = json!({"messages":[{"role":"user","content":[{"type":"text","text":"local fixture"}]},{"role":"assistant","content":null,"tool_calls":[{"id":"synthetic-call","type":"function","function":{"name":"fixture","arguments":"{\"value\":\"synthetic\"}"}}]}]});
        let mut map = dialogue_map("/messages/0/content/0/text");
        map.add(SourceEntry::dialogue(
            "call",
            SourceLocation::new("/messages/1/tool_calls"),
            DialoguePart::ToolCalls,
        ));
        let result = meter_request(&body, &map).unwrap();
        assert_reconciles(&result);
        assert_eq!(
            result.protocol_overhead.template_allowance_tokens,
            Some(16 + 8 * 2 + 8)
        );
        let text = result
            .categories
            .iter()
            .find(|row| row.category == DisplayCategory::CurrentConversation)
            .unwrap();
        assert!(
            text.children
                .as_ref()
                .unwrap()
                .iter()
                .find(|row| row.category == DialoguePart::ToolCalls)
                .unwrap()
                .estimated_tokens
                .unwrap()
                > 0
        );
    }
    #[test]
    fn embedded_split_labels_leave_structure_and_escaping_unchanged() {
        let prompt = json!({"instruction":"a\"b\\c\n中文", "state":{"nested":"fixture"}});
        let body = json!({"messages":[{"role":"user","content":prompt.to_string()}]});
        let mut map = RequestSourceMap::default();
        map.embedded_json.insert("/messages/0/content".into());
        map.add(SourceEntry::dialogue(
            "all",
            SourceLocation::embedded("/messages/0/content", ""),
            DialoguePart::OtherInputs,
        ));
        let original = meter_request(&body, &map).unwrap();
        map.entries.clear();
        for path in ["/instruction", "/state/nested"] {
            let text = prompt.pointer(path).unwrap().as_str().unwrap();
            for (start, c) in text.char_indices() {
                map.add(SourceEntry::dialogue(
                    "part",
                    SourceLocation::embedded("/messages/0/content", path)
                        .with_span(start, start + c.len_utf8()),
                    DialoguePart::OtherInputs,
                ));
            }
        }
        let split = meter_request(&body, &map).unwrap();
        assert_reconciles(&split);
        assert_eq!(split.input_tokens, original.input_tokens);
        assert_eq!(split.content_input_tokens, original.content_input_tokens);
        assert_eq!(
            split.protocol_overhead.estimated_tokens,
            original.protocol_overhead.estimated_tokens
        );
    }
    #[test]
    fn malformed_protocol_layout_is_a_total_failure() {
        let body = json!({"messages":[{"role":"user","content":"中文"}]});
        let mut map = dialogue_map("/messages/0/content");
        map.protocol_spans
            .push(SourceLocation::new("/messages/0/content").with_span(1, 2));
        assert!(meter_request(&body, &map).is_err());
        map.protocol_spans.clear();
        map.protocol_spans
            .push(SourceLocation::new("/messages/0/content").with_span(0, 3));
        map.protocol_spans
            .push(SourceLocation::new("/messages/0/content").with_span(0, 3));
        assert!(meter_request(&body, &map).is_err());
    }
    #[test]
    fn large_ascii_request_uses_source_ranges_and_preserves_totals() {
        let length = 1024 * 1024 + 1;
        let body = json!({"messages":[{"role":"user","content":"x".repeat(length)}]});
        let mut map = RequestSourceMap::default();
        for (start, end) in [(0, 1), (1, 1024), (1024, length)] {
            map.add(SourceEntry::dialogue(
                "fixture",
                SourceLocation::new("/messages/0/content").with_span(start, end),
                DialoguePart::UserMessages,
            ));
        }
        let result = meter_request(&body, &map).unwrap();
        assert_reconciles(&result);
        assert_eq!(result.content_input_tokens, (length as u64).div_ceil(2));
        let mut structure = Structure::default();
        structure.atomic(
            "/messages/0/content".into(),
            &body["messages"][0]["content"],
        );
        let ranges = source_ranges(&body, &map, &structure.units, &[Vec::new()]).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].len(), 3);
        // This asserts interval cardinality and results, not process RSS.
    }
    #[test]
    fn overlapping_labels_only_inside_protocol_do_not_conflict() {
        let body = json!({"messages":[{"role":"user","content":"prefix: content"}]});
        let mut map = dialogue_map("/messages/0/content");
        map.protocol_spans
            .push(SourceLocation::new("/messages/0/content").with_span(0, 8));
        map.add(SourceEntry::new(
            "wrapper",
            SourceLocation::new("/messages/0/content").with_span(0, 8),
            DisplayCategory::SystemInstructions,
        ));
        assert_reconciles(&meter_request(&body, &map).unwrap());
        map.entries[1].location = SourceLocation::new("/messages/0/content").with_span(0, 9);
        assert_eq!(
            meter_request(&body, &map)
                .unwrap()
                .classification_error
                .as_deref(),
            Some("overlapping_source_entries")
        );
    }
    #[test]
    fn managed_source_count_is_bounded_and_keeps_independent_total() {
        let body = json!({"messages":[{"role":"user","content":"fixture"}]});
        let baseline = meter_request(&body, &dialogue_map("/messages/0/content")).unwrap();
        let entry = SourceEntry::dialogue(
            "u",
            SourceLocation::new("/messages/0/content"),
            DialoguePart::UserMessages,
        );
        let mut map = RequestSourceMap::default();
        for _ in 0..MAX_SOURCE_ENTRIES {
            map.add(entry.clone());
        }
        assert_eq!(map.entries.len(), MAX_SOURCE_ENTRIES);
        let bytes = map.estimated_memory_bytes();
        map.add(entry.clone());
        map.add(entry);
        assert_eq!(map.entries.len(), MAX_SOURCE_ENTRIES);
        assert_eq!(map.estimated_memory_bytes(), bytes);
        let result = meter_request(&body, &map).unwrap();
        assert_eq!(
            result.classification_error.as_deref(),
            Some("source_map_limit_exceeded")
        );
        assert_eq!(result.input_tokens, baseline.input_tokens);
    }
    #[test]
    fn managed_layout_counts_are_bounded_and_fail_closed() {
        let body = json!({"messages":[]});
        let mut map = RequestSourceMap::default();
        for _ in 0..MAX_PROTOCOL_SPANS {
            map.add_protocol(SourceLocation::new("/messages"));
        }
        assert_eq!(map.protocol_spans.len(), MAX_PROTOCOL_SPANS);
        let bytes = map.estimated_memory_bytes();
        map.add_protocol(SourceLocation::new("/messages"));
        assert_eq!(map.protocol_spans.len(), MAX_PROTOCOL_SPANS);
        assert_eq!(map.estimated_memory_bytes(), bytes);
        assert!(meter_request(&body, &map).is_err());
        let mut roots = RequestSourceMap::default();
        for index in 0..MAX_LAYOUT_ROOTS {
            roots.add_embedded_json(format!("/messages/{index}/content"));
        }
        let bytes = roots.estimated_memory_bytes();
        roots.add_embedded_json("/one-too-many");
        assert_eq!(roots.embedded_json.len(), MAX_LAYOUT_ROOTS);
        assert_eq!(roots.estimated_memory_bytes(), bytes);
        assert!(meter_request(&body, &roots).is_err());
    }
    #[test]
    fn memory_estimate_counts_spare_slots_strings_and_tree_nodes() {
        let mut map = RequestSourceMap::default();
        assert_eq!(
            map.estimated_memory_bytes(),
            std::mem::size_of::<RequestSourceMap>()
        );
        map.entries = Vec::with_capacity(3);
        map.protocol_spans = Vec::with_capacity(2);
        let mut source_id = String::with_capacity(128);
        source_id.push('u');
        let mut pointer = String::with_capacity(256);
        pointer.push_str("/messages/0/content");
        let mut inner = String::with_capacity(512);
        inner.push_str("/fixture");
        map.entries.push(SourceEntry::dialogue(
            source_id,
            SourceLocation::embedded(pointer, inner),
            DialoguePart::UserMessages,
        ));
        map.protocol_spans.push(SourceLocation::new("/wrapper"));
        map.embedded_json.insert("/root".into());
        let expected = std::mem::size_of::<RequestSourceMap>()
            + map.entries.capacity() * std::mem::size_of::<SourceEntry>()
            + map.protocol_spans.capacity() * std::mem::size_of::<SourceLocation>()
            + entry_heap_bytes(&map.entries[0])
            + location_heap_bytes(&map.protocol_spans[0])
            + BTREE_ROOT_ALLOWANCE_BYTES
            + map.embedded_json.first().unwrap().capacity();
        assert_eq!(map.estimated_memory_bytes(), expected);
    }
    #[test]
    fn metadata_byte_limits_count_capacity_and_do_not_grow() {
        let body = json!({"messages":[]});
        let mut map = RequestSourceMap::default();
        let large_capacity = String::with_capacity(MAX_SOURCE_MAP_BYTES);
        map.add(SourceEntry::new(
            large_capacity,
            SourceLocation::new("/messages"),
            DisplayCategory::HistorySummary,
        ));
        assert!(map.entries.is_empty());
        assert!(map.estimated_memory_bytes() <= MAX_SOURCE_MAP_BYTES);
        assert_eq!(
            meter_request(&body, &map)
                .unwrap()
                .classification_error
                .as_deref(),
            Some("source_map_limit_exceeded")
        );
        let mut protocol = RequestSourceMap::default();
        protocol.add_protocol(SourceLocation::new(String::with_capacity(
            MAX_SOURCE_MAP_BYTES,
        )));
        assert!(protocol.protocol_spans.is_empty());
        assert!(meter_request(&body, &protocol).is_err());
    }
    #[test]
    fn manually_constructed_oversize_maps_are_checked_before_resolution() {
        let body = json!({"messages":[]});
        let mut map = RequestSourceMap {
            entries: vec![
                SourceEntry::new(
                    "fixture",
                    SourceLocation::new("/missing"),
                    DisplayCategory::FixedRules
                );
                MAX_SOURCE_ENTRIES + 1
            ],
            ..RequestSourceMap::default()
        };
        let result = meter_request(&body, &map).unwrap();
        assert_eq!(
            result.classification_error.as_deref(),
            Some("source_map_limit_exceeded")
        );
        assert!(result
            .categories
            .iter()
            .all(|row| row.estimated_tokens.is_none()));
        map.entries.clear();
        map.protocol_spans = vec![SourceLocation::new("/missing"); MAX_PROTOCOL_SPANS + 1];
        assert!(meter_request(&body, &map).is_err());
        let mut roots = RequestSourceMap::default();
        for index in 0..=MAX_LAYOUT_ROOTS {
            roots.embedded_json.insert(format!("/missing/{index}"));
        }
        assert!(meter_request(&body, &roots).is_err());
        let mut bytes = RequestSourceMap::default();
        bytes.entries.push(SourceEntry::new(
            String::with_capacity(MAX_SOURCE_MAP_BYTES),
            SourceLocation::new("/missing"),
            DisplayCategory::FixedRules,
        ));
        assert_eq!(
            meter_request(&body, &bytes)
                .unwrap()
                .classification_error
                .as_deref(),
            Some("source_map_limit_exceeded")
        );
    }
}
