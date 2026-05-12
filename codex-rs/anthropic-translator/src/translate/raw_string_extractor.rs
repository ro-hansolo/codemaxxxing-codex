//! Streaming extractor for the contents of `{"raw": "..."}`.
//!
//! Anthropic streams custom-tool input as `input_json_delta` chunks
//! that look like `{"r`, `aw":"hel`, `lo\\n`, `wor`, `ld"}`. This
//! extractor consumes those chunks one at a time and emits the
//! decoded contents of the `raw` string field as soon as the bytes
//! are committed, without buffering the whole JSON payload.
//!
//! The implementation is a minimal streaming JSON tokenizer that
//! locates the *top-level* `"raw"` member of the envelope object,
//! decodes its string value (including UTF-16 surrogate pairs per
//! [RFC 8259 §7](https://www.rfc-editor.org/rfc/rfc8259#section-7))
//! one chunk at a time, and ignores everything else. It tolerates:
//!
//!   * `raw` not being the first key (`{"explanation":"...",
//!     "raw":"..."}`) — Anthropic's `eager_input_streaming` does not
//!     fix key order, and the model freely adds rationale fields.
//!   * Other key types preceding `raw` — strings (with embedded
//!     escapes), nested objects/arrays, numbers, `true`/`false`/`null`.
//!   * Chunk boundaries that split any token, including mid-`\uXXXX`
//!     escapes and between the high and low half of a surrogate pair.
//!   * Lone (unpaired) surrogates are dropped rather than crashing
//!     or corrupting subsequent bytes.

const RAW_KEY: &[u8] = b"raw";
const HIGH_SURROGATE_RANGE: std::ops::RangeInclusive<u16> = 0xD800..=0xDBFF;
const LOW_SURROGATE_RANGE: std::ops::RangeInclusive<u16> = 0xDC00..=0xDFFF;
const SUPPLEMENTARY_BASE: u32 = 0x1_0000;
const HIGH_SURROGATE_OFFSET: u32 = 0xD800;
const LOW_SURROGATE_OFFSET: u32 = 0xDC00;
const SURROGATE_HIGH_SHIFT: u32 = 10;

/// Per-tool-call extraction state.
pub struct RawStringExtractor {
    state: State,
    /// Bytes carried over from a prior chunk that ended mid-token
    /// (mid-`\uXXXX`, between the halves of a surrogate pair, etc.).
    /// Always small (≤ ~12 chars).
    pending: String,
    /// Pending UTF-16 high surrogate from a `\uXXXX` escape inside
    /// the `raw` value, awaiting its low-surrogate counterpart.
    high_surrogate: Option<u16>,
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    /// Initial: skipping leading whitespace, waiting for `{`.
    AwaitingObject,
    /// Inside `{...}` between members: waiting for `"` (next key) or
    /// `}` (end of object).
    BetweenMembers,
    /// Reading a top-level key character-by-character. `matched` is
    /// the next byte index in `RAW_KEY` to compare; `still_matching`
    /// flips to `false` on the first mismatch.
    InKey {
        matched: usize,
        still_matching: bool,
        escape: bool,
    },
    /// Just consumed the closing `"` of a key; waiting for `:`.
    AfterKey { is_raw: bool },
    /// Just consumed `:`; waiting for the value to start.
    BeforeValue { is_raw: bool },
    /// Inside the `raw` value string. Decoded bytes are emitted as
    /// they're committed.
    InRawValue,
    /// Skipping a non-raw string value.
    SkipString { escape: bool },
    /// Skipping a non-raw scalar (number/`true`/`false`/`null`)
    /// until a terminator.
    SkipScalar,
    /// Skipping a non-raw nested object/array.
    SkipNested {
        depth: u32,
        in_string: bool,
        escape: bool,
    },
    /// Just consumed a value; waiting for `,` or `}`.
    AfterValue,
    /// Done: subsequent bytes are ignored.
    Done,
}

/// Result of advancing the state machine by one input character.
enum StepOutcome {
    /// Advanced state and consumed `n` characters from the input.
    /// `n == 0` is reserved for "transitioned without consuming"
    /// and is not used by any current branch (avoids re-entry
    /// loops); always `n == 1` or a small fixed window for `\uXXXX`.
    Consumed(usize),
    /// State machine needs more bytes than the current chunk
    /// provides (e.g. only saw `\u00` so far). Caller must buffer
    /// the unconsumed tail and wait for the next push.
    NeedMore,
}

impl RawStringExtractor {
    pub fn new() -> Self {
        Self {
            state: State::AwaitingObject,
            pending: String::new(),
            high_surrogate: None,
        }
    }

    /// Push the next JSON delta chunk and return any decoded raw
    /// bytes. Returns an empty string when the chunk only advanced
    /// internal state (no new raw content yet).
    pub fn push(&mut self, chunk: &str) -> String {
        if matches!(self.state, State::Done) {
            return String::new();
        }
        let mut out = String::new();
        let combined = self.combine_pending(chunk);
        let chars: Vec<char> = combined.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if matches!(self.state, State::Done) {
                break;
            }
            match self.step(&chars, i, &mut out) {
                StepOutcome::Consumed(n) => {
                    debug_assert!(n > 0, "step() must consume at least one char");
                    i += n;
                }
                StepOutcome::NeedMore => {
                    self.pending = chars[i..].iter().collect();
                    return out;
                }
            }
        }
        out
    }

    fn combine_pending(&mut self, chunk: &str) -> String {
        if self.pending.is_empty() {
            chunk.to_string()
        } else {
            let mut s = std::mem::take(&mut self.pending);
            s.push_str(chunk);
            s
        }
    }

    fn step(&mut self, chars: &[char], i: usize, out: &mut String) -> StepOutcome {
        let ch = chars[i];
        match &mut self.state {
            State::AwaitingObject => {
                if ch == '{' {
                    self.state = State::BetweenMembers;
                }
                // Whitespace and any other forward-compat byte just
                // get consumed.
                StepOutcome::Consumed(1)
            }
            State::BetweenMembers => match ch {
                c if c.is_whitespace() || c == ',' => StepOutcome::Consumed(1),
                '"' => {
                    self.state = State::InKey {
                        matched: 0,
                        still_matching: true,
                        escape: false,
                    };
                    StepOutcome::Consumed(1)
                }
                '}' => {
                    self.state = State::Done;
                    StepOutcome::Consumed(1)
                }
                _ => StepOutcome::Consumed(1),
            },
            State::InKey {
                matched,
                still_matching,
                escape,
            } => {
                if *escape {
                    // Any escaped char inside a key is enough to
                    // disqualify the key from matching `raw`.
                    *escape = false;
                    *still_matching = false;
                    return StepOutcome::Consumed(1);
                }
                match ch {
                    '\\' => {
                        *escape = true;
                        StepOutcome::Consumed(1)
                    }
                    '"' => {
                        let is_raw = *still_matching && *matched == RAW_KEY.len();
                        self.state = State::AfterKey { is_raw };
                        StepOutcome::Consumed(1)
                    }
                    other => {
                        if *still_matching {
                            // Compare against the ASCII bytes of
                            // RAW_KEY. A non-ASCII char in the key
                            // can't match (RAW_KEY is ASCII-only),
                            // so flip still_matching off.
                            let is_ascii = other.is_ascii();
                            if is_ascii
                                && *matched < RAW_KEY.len()
                                && (other as u32) == u32::from(RAW_KEY[*matched])
                            {
                                *matched += 1;
                            } else {
                                *still_matching = false;
                            }
                        }
                        StepOutcome::Consumed(1)
                    }
                }
            }
            State::AfterKey { is_raw } => {
                let is_raw = *is_raw;
                if ch == ':' {
                    self.state = State::BeforeValue { is_raw };
                }
                StepOutcome::Consumed(1)
            }
            State::BeforeValue { is_raw } => {
                let is_raw = *is_raw;
                match ch {
                    c if c.is_whitespace() => StepOutcome::Consumed(1),
                    '"' => {
                        self.state = if is_raw {
                            State::InRawValue
                        } else {
                            State::SkipString { escape: false }
                        };
                        StepOutcome::Consumed(1)
                    }
                    '{' | '[' => {
                        // Nested object/array as a value (only valid
                        // for non-`raw` keys per the schema). If it
                        // somehow appears for `raw`, skipping
                        // gracefully is safer than crashing.
                        self.state = State::SkipNested {
                            depth: 1,
                            in_string: false,
                            escape: false,
                        };
                        StepOutcome::Consumed(1)
                    }
                    _ => {
                        // Number, `true`, `false`, or `null`. Treat
                        // the current char as the first byte of the
                        // scalar.
                        self.state = State::SkipScalar;
                        StepOutcome::Consumed(1)
                    }
                }
            }
            State::InRawValue => self.step_in_raw_value(chars, i, out),
            State::SkipString { escape } => {
                if *escape {
                    *escape = false;
                    return StepOutcome::Consumed(1);
                }
                match ch {
                    '\\' => {
                        *escape = true;
                        StepOutcome::Consumed(1)
                    }
                    '"' => {
                        self.state = State::AfterValue;
                        StepOutcome::Consumed(1)
                    }
                    _ => StepOutcome::Consumed(1),
                }
            }
            State::SkipScalar => match ch {
                ',' => {
                    self.state = State::BetweenMembers;
                    StepOutcome::Consumed(1)
                }
                '}' => {
                    self.state = State::Done;
                    StepOutcome::Consumed(1)
                }
                c if c.is_whitespace() => {
                    self.state = State::AfterValue;
                    StepOutcome::Consumed(1)
                }
                _ => StepOutcome::Consumed(1),
            },
            State::SkipNested {
                depth,
                in_string,
                escape,
            } => {
                if *in_string {
                    if *escape {
                        *escape = false;
                    } else if ch == '\\' {
                        *escape = true;
                    } else if ch == '"' {
                        *in_string = false;
                    }
                    return StepOutcome::Consumed(1);
                }
                match ch {
                    '"' => {
                        *in_string = true;
                        StepOutcome::Consumed(1)
                    }
                    '{' | '[' => {
                        *depth += 1;
                        StepOutcome::Consumed(1)
                    }
                    '}' | ']' => {
                        *depth -= 1;
                        if *depth == 0 {
                            self.state = State::AfterValue;
                        }
                        StepOutcome::Consumed(1)
                    }
                    _ => StepOutcome::Consumed(1),
                }
            }
            State::AfterValue => match ch {
                c if c.is_whitespace() => StepOutcome::Consumed(1),
                ',' => {
                    self.state = State::BetweenMembers;
                    StepOutcome::Consumed(1)
                }
                '}' => {
                    self.state = State::Done;
                    StepOutcome::Consumed(1)
                }
                _ => StepOutcome::Consumed(1),
            },
            State::Done => StepOutcome::Consumed(1),
        }
    }

    fn step_in_raw_value(&mut self, chars: &[char], i: usize, out: &mut String) -> StepOutcome {
        let ch = chars[i];
        match ch {
            '"' => {
                // End of the raw value. Drop any orphan high
                // surrogate per RFC 8259 §7 ("\uXXXX must form a
                // valid UTF-16 sequence").
                self.high_surrogate = None;
                self.state = State::Done;
                StepOutcome::Consumed(1)
            }
            '\\' => self.consume_escape(chars, i, out),
            other => {
                // Any prior pending high surrogate is now orphaned.
                self.high_surrogate = None;
                out.push(other);
                StepOutcome::Consumed(1)
            }
        }
    }

    fn consume_escape(&mut self, chars: &[char], i: usize, out: &mut String) -> StepOutcome {
        // Need at least one char after the backslash to know the
        // escape kind.
        let Some(esc) = chars.get(i + 1).copied() else {
            return StepOutcome::NeedMore;
        };
        if esc == 'u' {
            // Unicode escape: need 4 hex digits.
            const UNICODE_ESCAPE_LEN: usize = 6;
            if chars.len() < i + UNICODE_ESCAPE_LEN {
                return StepOutcome::NeedMore;
            }
            let hex: String = chars[i + 2..i + UNICODE_ESCAPE_LEN].iter().collect();
            let Ok(code) = u16::from_str_radix(&hex, 16) else {
                // Malformed escape — drop and keep the high
                // surrogate slot clear.
                self.high_surrogate = None;
                return StepOutcome::Consumed(UNICODE_ESCAPE_LEN);
            };
            self.absorb_unicode_escape(code, out);
            StepOutcome::Consumed(UNICODE_ESCAPE_LEN)
        } else {
            // Single-char escape — orphans any pending high
            // surrogate.
            self.high_surrogate = None;
            match esc {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\u{0008}'),
                'f' => out.push('\u{000C}'),
                _ => {
                    // Unknown escape — preserve verbatim rather
                    // than silently drop, mirroring Anthropic's
                    // current pass-through behaviour for the model's
                    // own raw output.
                    out.push('\\');
                    out.push(esc);
                }
            }
            StepOutcome::Consumed(2)
        }
    }

    fn absorb_unicode_escape(&mut self, code: u16, out: &mut String) {
        if HIGH_SURROGATE_RANGE.contains(&code) {
            // Stash the high surrogate; orphan any prior pending
            // high (the previous one had no low to pair with).
            self.high_surrogate = Some(code);
            return;
        }
        if LOW_SURROGATE_RANGE.contains(&code) {
            if let Some(high) = self.high_surrogate.take()
                && let Some(decoded) = decode_surrogate_pair(high, code)
            {
                out.push(decoded);
            }
            // Lone low surrogate: drop per RFC 8259 §7.
            return;
        }
        // BMP scalar.
        self.high_surrogate = None;
        if let Some(decoded) = char::from_u32(u32::from(code)) {
            out.push(decoded);
        }
    }
}

impl Default for RawStringExtractor {
    fn default() -> Self {
        Self::new()
    }
}

fn decode_surrogate_pair(high: u16, low: u16) -> Option<char> {
    let high_offset = u32::from(high - HIGH_SURROGATE_OFFSET as u16);
    let low_offset = u32::from(low - LOW_SURROGATE_OFFSET as u16);
    let scalar = SUPPLEMENTARY_BASE + (high_offset << SURROGATE_HIGH_SHIFT) + low_offset;
    char::from_u32(scalar)
}

#[cfg(test)]
mod tests {
    use super::RawStringExtractor;

    #[test]
    fn whole_payload_in_one_chunk() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"raw\":\"hello world\"}");
        assert_eq!(out, "hello world");
    }

    #[test]
    fn chunk_ending_at_colon_carries_through_to_next_chunk() {
        let mut x = RawStringExtractor::new();
        assert_eq!(x.push("{\"raw\":"), "");
        assert_eq!(x.push("\"hello\"}"), "hello");
    }

    #[test]
    fn chunk_ending_at_backslash_carries_through_to_next_chunk() {
        let mut x = RawStringExtractor::new();
        assert_eq!(x.push("{\"raw\":\"line one\\"), "line one");
        assert_eq!(x.push("ntwo\"}"), "\ntwo");
    }

    #[test]
    fn chunk_ending_mid_unicode_escape_carries_through() {
        let mut x = RawStringExtractor::new();
        assert_eq!(x.push("{\"raw\":\"x\\u00"), "x");
        assert_eq!(x.push("41y\"}"), "Ay");
    }

    #[test]
    fn json_escapes_decode_correctly() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"raw\":\"a\\nb\\tc\\\\d\\\"e\\/f\"}");
        assert_eq!(out, "a\nb\tc\\d\"e/f");
    }

    #[test]
    fn many_tiny_chunks_reproduce_payload() {
        let mut x = RawStringExtractor::new();
        let mut out = String::new();
        for chunk in [
            "{", "\"", "r", "a", "w", "\"", ":", "\"", "h", "i", "\"", "}",
        ] {
            out.push_str(&x.push(chunk));
        }
        assert_eq!(out, "hi");
    }

    #[test]
    fn whitespace_between_colon_and_quote_is_tolerated() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"raw\":   \"hello\"}");
        assert_eq!(out, "hello");
    }

    // -------------------------------------------------------------------
    // Surrogate pair handling
    //
    // Per RFC 8259 §7 ("Strings"): JSON encodes Unicode code points
    // outside the BMP as a UTF-16 surrogate pair, e.g. U+1F600 (😀)
    // is `\uD83D\uDE00`. `char::from_u32` rejects the individual
    // halves because they are not valid Unicode scalar values, so any
    // implementation that calls it once per `\uXXXX` sequence drops
    // every emoji, CJK supplementary char, etc. that arrives in an
    // apply_patch payload.
    //
    // Source of truth: RFC 8259 §7
    // <https://www.rfc-editor.org/rfc/rfc8259#section-7>
    // -------------------------------------------------------------------

    #[test]
    fn surrogate_pair_decodes_to_supplementary_code_point() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"raw\":\"x\\uD83D\\uDE00y\"}");
        assert_eq!(out, "x\u{1F600}y");
    }

    #[test]
    fn surrogate_pair_split_across_chunk_boundaries() {
        let mut x = RawStringExtractor::new();
        let mut out = String::new();
        out.push_str(&x.push("{\"raw\":\"a\\uD83D"));
        out.push_str(&x.push("\\uDE00"));
        out.push_str(&x.push("b\"}"));
        assert_eq!(out, "a\u{1F600}b");
    }

    #[test]
    fn surrogate_pair_split_inside_low_surrogate_escape() {
        let mut x = RawStringExtractor::new();
        let mut out = String::new();
        out.push_str(&x.push("{\"raw\":\"a\\uD83D\\uDE"));
        out.push_str(&x.push("00b\"}"));
        assert_eq!(out, "a\u{1F600}b");
    }

    #[test]
    fn lone_high_surrogate_at_end_of_value_is_dropped_safely() {
        // A high surrogate not followed by `\uXXXX` is malformed JSON.
        // Drop it rather than corrupting the rest of the stream.
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"raw\":\"a\\uD83Db\"}");
        assert_eq!(out, "ab");
    }

    #[test]
    fn lone_low_surrogate_is_dropped_safely() {
        // Low surrogate without a preceding high one: drop, don't
        // panic.
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"raw\":\"a\\uDE00b\"}");
        assert_eq!(out, "ab");
    }

    // -------------------------------------------------------------------
    // Key-aware extraction
    //
    // Anthropic's `eager_input_streaming` does not constrain the model
    // to emit `"raw"` as the first key of the streamed object. When
    // the model adds rationale fields (e.g. `{"explanation":"...",
    // "raw":"..."}`), the previous "first colon then first string"
    // heuristic streamed the explanation and silently dropped the
    // patch body. We must only emit the value of the literal `"raw"`
    // key, regardless of position.
    //
    // Source of truth: Anthropic fine-grained tool streaming docs
    // <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/fine-grained-tool-streaming>
    // -------------------------------------------------------------------

    #[test]
    fn raw_value_extracted_even_when_preceded_by_other_string_key() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"explanation\":\"fixing bug\",\"raw\":\"actual code\"}");
        assert_eq!(out, "actual code");
    }

    #[test]
    fn raw_value_extracted_when_preceded_by_object_key() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"meta\":{\"reason\":\"foo\"},\"raw\":\"actual code\"}");
        assert_eq!(out, "actual code");
    }

    #[test]
    fn raw_value_extracted_when_preceded_by_array_key() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"tags\":[\"a\",\"b\"],\"raw\":\"actual code\"}");
        assert_eq!(out, "actual code");
    }

    #[test]
    fn raw_value_extracted_when_preceded_by_numeric_key() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"count\":42,\"raw\":\"actual code\"}");
        assert_eq!(out, "actual code");
    }

    #[test]
    fn raw_value_extracted_when_preceded_by_bool_and_null_keys() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"ok\":true,\"err\":null,\"flag\":false,\"raw\":\"actual code\"}");
        assert_eq!(out, "actual code");
    }

    #[test]
    fn trailing_keys_after_raw_are_ignored() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"raw\":\"actual code\",\"meta\":\"ignored\"}");
        assert_eq!(out, "actual code");
    }

    #[test]
    fn payload_with_only_non_raw_keys_emits_nothing() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"explanation\":\"no raw here\"}");
        assert_eq!(out, "");
    }

    #[test]
    fn keys_with_escapes_are_skipped_correctly() {
        // The non-raw key contains an embedded `"` via escape so a
        // naive "next quote ends the key" parser would misread the
        // structure.
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"label\\\"x\":\"foo\",\"raw\":\"actual code\"}");
        assert_eq!(out, "actual code");
    }

    #[test]
    fn key_split_across_chunk_boundary_still_matches_raw() {
        let mut x = RawStringExtractor::new();
        let mut out = String::new();
        out.push_str(&x.push("{\"explanation\":\"x\",\"r"));
        out.push_str(&x.push("aw\":\"hi\"}"));
        assert_eq!(out, "hi");
    }

    #[test]
    fn key_named_raw_inside_nested_object_does_not_match() {
        // Only the top-level `raw` key matters. A nested `raw`
        // shouldn't trigger streaming — otherwise rationale objects
        // like `{"meta":{"raw":"description"}, "raw":"code"}` would
        // emit the wrong value.
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"meta\":{\"raw\":\"description\"},\"raw\":\"code\"}");
        assert_eq!(out, "code");
    }

    #[test]
    fn skipping_string_value_with_escaped_quote_does_not_close_early() {
        let mut x = RawStringExtractor::new();
        let out = x.push("{\"explanation\":\"has \\\"quote\\\" inside\",\"raw\":\"hi\"}");
        assert_eq!(out, "hi");
    }
}
