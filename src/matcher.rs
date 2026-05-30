// This file is part of the uutils grep package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

use crate::{Config, RegexMode};
use onig::{
    EncodedBytes, Error, MatchParam, Regex, RegexOptions, Region, SearchOptions, Syntax,
    SyntaxBehavior, SyntaxOperator,
};
use onig_sys::{
    ONIGERR_EMPTY_RANGE_IN_CHAR_CLASS, ONIGERR_INVALID_BACKREF, ONIGERR_RETRY_LIMIT_IN_MATCH_OVER,
    ONIGERR_RETRY_LIMIT_IN_SEARCH_OVER, OnigEncCtype_ONIGENC_CTYPE_WORD, OnigEncodingUTF8,
};
use std::io;
use uucore::error::{UResult, USimpleError};

pub struct Matcher<'a> {
    config: &'a Config<'a>,
    patterns: Vec<CompiledPattern>,
}

impl<'a> Matcher<'a> {
    pub fn compile(config: &'a Config<'a>) -> UResult<Self> {
        let mut patterns = Vec::with_capacity(config.patterns.len());
        for raw in config.patterns {
            patterns.push(CompiledPattern::compile(raw, config)?);
        }
        Ok(Self { config, patterns })
    }

    /// Decide whether `line` matches and return the positions to highlight.
    ///
    /// Returns an error if the regex engine bails out (e.g. it exceeds its
    /// backtracking retry limit on a pathological pattern); the caller turns
    /// that into a GNU-style diagnostic and exit code 2 rather than aborting.
    pub fn match_line(&self, line: &[u8]) -> io::Result<Option<Vec<(usize, usize)>>> {
        let mut any_seen = false;
        let mut positions = Vec::new();
        let mut iter = MatchIter::new(&self.patterns, line).map_err(match_error)?;
        while let Some((start, end)) = iter.next_match().map_err(match_error)? {
            any_seen = true;
            // Drop zero-length matches from the output.
            if start == end {
                continue;
            }
            // Drop matches that don't span the whole line if `-x` was requested.
            if self.config.line_regexp && !(start == 0 && end == line.len()) {
                continue;
            }
            // Drop matches that aren't word matches if `-w` was requested.
            if self.config.word_regexp && !Self::is_word_match(line, start, end) {
                continue;
            }
            positions.push((start, end));
        }

        let raw_matched = if self.config.line_regexp || self.config.word_regexp {
            // -w / -x are authoritative once positions are filtered.
            !positions.is_empty()
        } else {
            any_seen
        };

        Ok((raw_matched != self.config.invert_match).then_some(positions))
    }

    /// Cheap match check that doesn't enumerate positions.
    pub fn is_match(&self, line: &[u8]) -> io::Result<Option<Vec<(usize, usize)>>> {
        // `-w` / `-x` need positions to filter, so we fall back to `match_line`.
        let matched = if self.config.line_regexp || self.config.word_regexp {
            self.match_line(line)?.is_some()
        } else {
            let mut raw_matched = false;
            for p in &self.patterns {
                if p.is_match(line).map_err(match_error)? {
                    raw_matched = true;
                    break;
                }
            }
            raw_matched != self.config.invert_match
        };
        Ok(matched.then(Vec::new))
    }

    /// Word-boundary check `-w`.
    /// NOTE that `-w` does not check both sides, unlike `\b` in a regex.
    /// Start/End-of-line count as non-words.
    fn is_word_match(line: &[u8], start: usize, end: usize) -> bool {
        // SAFETY: This code uses OnigEncodingType such that it can support other types of encodings in the future.
        unsafe {
            let mbc_to_code = OnigEncodingUTF8.mbc_to_code.unwrap_unchecked();
            let is_code_ctype = OnigEncodingUTF8.is_code_ctype.unwrap_unchecked();
            let line_end = line.as_ptr().add(line.len());

            if end < line.len() {
                let cp = mbc_to_code(line.as_ptr().add(end), line_end);
                if is_code_ctype(cp, OnigEncCtype_ONIGENC_CTYPE_WORD) != 0 {
                    return false;
                }
            }

            if start > 0 {
                let left_adjust = OnigEncodingUTF8.left_adjust_char_head.unwrap_unchecked();
                let head = left_adjust(line.as_ptr(), line.as_ptr().add(start - 1));
                let cp = mbc_to_code(head, line_end);
                if is_code_ctype(cp, OnigEncCtype_ONIGENC_CTYPE_WORD) != 0 {
                    return false;
                }
            }

            true
        }
    }
}

/// Streaming k-way merge over compiled patterns
struct MatchIter<'a> {
    cursors: Vec<Cursor<'a>>,
    /// End of the last emitted match.
    last_end: usize,
}

impl<'a> MatchIter<'a> {
    fn new(patterns: &'a [CompiledPattern], line: &'a [u8]) -> Result<Self, Error> {
        let mut cursors = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let mut c = Cursor {
                pattern,
                line,
                offset: 0,
                pending: None,
            };
            c.refill()?;
            cursors.push(c);
        }
        Ok(Self {
            cursors,
            last_end: 0,
        })
    }

    /// Yield the next match across all patterns, or `None` when exhausted.
    fn next_match(&mut self) -> Result<Option<(usize, usize)>, Error> {
        // Discard stale pendings that fall before the last emit.
        for cursor in &mut self.cursors {
            if matches!(cursor.pending, Some((s, _)) if s < self.last_end) {
                cursor.offset = self.last_end;
                cursor.refill()?;
            }
        }

        // Pick the leftmost pending.
        // Tie-break by largest end so POSIX leftmost-longest holds across
        // patterns too (e.g. `-e a -e ab` against `ab` emits `ab`).
        let best_idx = self
            .cursors
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.pending.map(|p| (i, p)))
            .min_by_key(|&(_, (s, e))| (s, std::cmp::Reverse(e)))
            .map(|(i, _)| i);
        let Some(best_idx) = best_idx else {
            return Ok(None);
        };

        let (start, end) = self.cursors[best_idx].pending.unwrap();
        self.cursors[best_idx].refill()?;
        self.last_end = end;
        Ok(Some((start, end)))
    }
}

struct Cursor<'a> {
    pattern: &'a CompiledPattern,
    line: &'a [u8],
    /// Where the next `search_leftmost` call should start.
    offset: usize,
    /// Pre-fetched next match for this pattern.
    /// `None` once the pattern is exhausted.
    pending: Option<(usize, usize)>,
}

impl Cursor<'_> {
    fn refill(&mut self) -> Result<(), Error> {
        if self.offset >= self.line.len() {
            self.pending = None;
            return Ok(());
        }
        let Some((start, leftmost_end)) = self.pattern.search_leftmost(self.line, self.offset)?
        else {
            self.pending = None;
            return Ok(());
        };
        let end = self
            .pattern
            .longest_end_at(self.line, start)?
            .unwrap_or(leftmost_end);
        // Advance the next search past the match we just found.
        // Zero-length matches need a +1 nudge to avoid spinning forever.
        self.offset = end.max(start + 1);
        self.pending = Some((start, end));
        Ok(())
    }
}

struct CompiledPattern {
    /// Default semantics. It's decently fast and used for searching.
    leftmost: Regex,
    /// Compiled with `FIND_LONGEST`. If used for a search, it'll search the
    /// entire haystack to find the longest. This makes it unsuitable for searching,
    /// but it's perfect for a second, anchored match pass for POSIX semantics.
    longest_anchored: Regex,
}

impl CompiledPattern {
    fn compile(pattern: &str, config: &Config) -> UResult<Self> {
        // GNU grep rejects the confusing `[:name:]` bracket form (a misspelled
        // `[[:name:]]`) in basic/extended modes; oniguruma accepts it silently.
        if matches!(config.regex_mode, RegexMode::Basic | RegexMode::Extended) {
            check_confusing_bracket(pattern)?;
        }

        let mut syntax = *match config.regex_mode {
            RegexMode::Fixed => Syntax::asis(),
            RegexMode::Basic => Syntax::grep(),
            RegexMode::Extended => Syntax::gnu_regex(),
            RegexMode::Perl => Syntax::perl_ng(),
        };
        if config.regex_mode != RegexMode::Fixed {
            // GNU grep supports `{,n}` as an alias for `{0,n}`.
            syntax.enable_behavior(SyntaxBehavior::SYNTAX_BEHAVIOR_ALLOW_INTERVAL_LOW_ABBREV);
        }
        if config.regex_mode == RegexMode::Perl {
            // GNU grep supports `(?P<name>...)`.
            // Unfortunately, the onig crate defines the OP2 flag without the
            // necessary <<32 bit shift, so we need to hotpatch that here.
            const _: () =
                assert!(SyntaxOperator::SYNTAX_OPERATOR_QMARK_CAPITAL_P_NAME.bits() == 0x80000000);
            const FIXED: SyntaxOperator = SyntaxOperator::from_bits_retain(
                SyntaxOperator::SYNTAX_OPERATOR_QMARK_CAPITAL_P_NAME.bits() << 32,
            );
            syntax.enable_operators(FIXED);
        }

        let mut options = RegexOptions::REGEX_OPTION_NONE;
        if config.ignore_case {
            options |= RegexOptions::REGEX_OPTION_IGNORECASE;
        }

        let mode = config.regex_mode;
        fn compile_with(
            pattern: &str,
            syntax: &Syntax,
            options: RegexOptions,
            mode: RegexMode,
        ) -> UResult<Regex> {
            Regex::with_options_and_encoding(pattern, options, syntax).map_err(|err| {
                // Prefer GNU grep's wording for the errors it has a dedicated
                // message for; fall back to oniguruma's text otherwise.
                match gnu_error_message(err.code(), mode) {
                    Some(msg) => USimpleError::new(2, msg.to_string()),
                    None => USimpleError::new(2, format!("invalid pattern \"{pattern}\": {err}")),
                }
            })
        }

        let leftmost = compile_with(pattern, &syntax, options, mode)?;
        let longest_anchored = compile_with(
            pattern,
            &syntax,
            options | RegexOptions::REGEX_OPTION_FIND_LONGEST,
            mode,
        )?;
        Ok(Self {
            leftmost,
            longest_anchored,
        })
    }

    /// Find the leftmost match starting at or after `offset`.
    fn search_leftmost(&self, line: &[u8], offset: usize) -> Result<Option<(usize, usize)>, Error> {
        let mut region = Region::new();
        let found = self.leftmost.search_with_param(
            EncodedBytes::from_parts(line, &raw mut OnigEncodingUTF8),
            offset,
            line.len(),
            SearchOptions::SEARCH_OPTION_NONE,
            Some(&mut region),
            MatchParam::default(),
        )?;
        Ok(found.and_then(|_| region.pos(0)))
    }

    /// Given a known leftmost start `start`, return the longest extent
    /// of a match anchored exactly there = POSIX leftmost-longest end.
    fn longest_end_at(&self, line: &[u8], start: usize) -> Result<Option<usize>, Error> {
        let mut region = Region::new();
        self.longest_anchored.match_with_param(
            EncodedBytes::from_parts(line, &raw mut OnigEncodingUTF8),
            start,
            SearchOptions::SEARCH_OPTION_NONE,
            Some(&mut region),
            MatchParam::default(),
        )?;
        Ok(region.pos(0).map(|(_, end)| end))
    }

    /// True if any match exists in `line` (including zero-length).
    fn is_match(&self, line: &[u8]) -> Result<bool, Error> {
        Ok(self
            .leftmost
            .search_with_param(
                EncodedBytes::from_parts(line, &raw mut OnigEncodingUTF8),
                0,
                line.len(),
                SearchOptions::SEARCH_OPTION_NONE,
                None,
                MatchParam::default(),
            )?
            .is_some())
    }
}

/// Convert a regex-engine match-time error into an I/O error carrying GNU
/// grep's wording. The only error we expect in practice is the backtracking
/// retry limit being exceeded on a pathological pattern; GNU reports this as
/// `exceeded PCRE's backtracking limit` and exits 2 instead of aborting.
fn match_error(err: Error) -> io::Error {
    let message = if matches!(
        err.code(),
        ONIGERR_RETRY_LIMIT_IN_MATCH_OVER | ONIGERR_RETRY_LIMIT_IN_SEARCH_OVER
    ) {
        "exceeded PCRE's backtracking limit".to_string()
    } else {
        err.description().to_string()
    };
    io::Error::other(message)
}

/// Map an oniguruma compile-error code to GNU grep's wording for the same
/// condition, when one exists. GNU emits a bare POSIX-style diagnostic (e.g.
/// `Invalid range end`) rather than oniguruma's phrasing, so translating keeps
/// us byte-compatible. Returns `None` for errors with no GNU equivalent, where
/// the caller falls back to oniguruma's own message.
fn gnu_error_message(code: i32, mode: RegexMode) -> Option<&'static str> {
    match code {
        // e.g. `[b-a]`: a range whose end precedes its start.
        ONIGERR_EMPTY_RANGE_IN_CHAR_CLASS => Some("Invalid range end"),
        // e.g. `(.)\2`: a back-reference to a group that does not exist. GNU
        // (via PCRE2) and gnulib's regex word this differently.
        ONIGERR_INVALID_BACKREF if mode == RegexMode::Perl => {
            Some("reference to non-existent subpattern")
        }
        ONIGERR_INVALID_BACKREF => Some("Invalid back reference"),
        _ => None,
    }
}

/// Reject the confusing `[:name:]` bracket form the way GNU grep does.
///
/// A bracket expression like `[:space:]` is almost always a misspelled
/// `[[:space:]]`; GNU grep flags it with a dedicated diagnostic and exits 2,
/// whereas oniguruma silently treats it as the set `{':','s','p',…}`. This
/// scans the pattern for that form and returns the same error.
fn check_confusing_bracket(pattern: &str) -> UResult<()> {
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // Outside a bracket a backslash escapes the next character, so
            // `\[` does not open a bracket expression.
            b'\\' => i += 2,
            b'[' => {
                i += 1;
                if bracket_warns(bytes, &mut i) {
                    return Err(USimpleError::new(
                        2,
                        "character class syntax is [[:space:]], not [:space:]".to_string(),
                    ));
                }
            }
            _ => i += 1,
        }
    }
    Ok(())
}

/// Consume a single bracket expression starting just past its opening `[` and
/// report whether GNU grep's colon warning fires for it.
///
/// This is a faithful port of the `colon_warning_state` logic in GNU grep's
/// `parse_bracket_exp` (gnulib `dfa.c`). The state is a bitmask:
///   bit 0 — first character is a colon
///   bit 1 — last character is a colon
///   bit 2 — includes some other (non-colon) character
///   bit 3 — includes a range, char/equivalence class, or collating element
/// The warning fires exactly when the state ends equal to `7` (bits 0–2 set,
/// bit 3 clear). On the way it advances `i` past the closing `]`.
fn bracket_warns(bytes: &[u8], i: &mut usize) -> bool {
    fn fetch(bytes: &[u8], i: &mut usize) -> Option<u8> {
        let b = bytes.get(*i).copied();
        if b.is_some() {
            *i += 1;
        }
        b
    }

    let Some(first) = fetch(bytes, i) else {
        return false;
    };
    let mut c = first;
    if c == b'^' {
        match fetch(bytes, i) {
            Some(x) => c = x,
            None => return false,
        }
    }
    let mut state: u8 = u8::from(c == b':');

    'scan: loop {
        state &= !2;
        let mut c1: Option<u8> = None;

        if c == b'[' {
            let Some(nc1) = fetch(bytes, i) else {
                return false;
            };
            // `[:`, `[.` and `[=` introduce a class / collating / equivalence
            // element; consume it whole and mark bit 3.
            if nc1 == b':' || nc1 == b'.' || nc1 == b'=' {
                loop {
                    match fetch(bytes, i) {
                        None => break,
                        Some(cc) if cc == nc1 && bytes.get(*i).copied() == Some(b']') => break,
                        Some(_) => {}
                    }
                }
                if fetch(bytes, i).is_none() {
                    return false; // consumes the `]`
                }
                state |= 8;
                match fetch(bytes, i) {
                    Some(b']') => break 'scan,
                    Some(x) => {
                        c = x;
                        continue 'scan;
                    }
                    None => return false,
                }
            }
            // Otherwise `[` is an ordinary character; `nc1` is the lookahead.
            c1 = Some(nc1);
        }

        if c1.is_none() {
            c1 = fetch(bytes, i);
        }

        if c1 == Some(b'-') {
            let Some(mut c2) = fetch(bytes, i) else {
                return false;
            };
            if c2 == b'[' && bytes.get(*i).copied() == Some(b'.') {
                c2 = b']';
            }
            if c2 == b']' {
                // `[x-]`: the hyphen is a literal; put the `]` back so the
                // loop terminator sees it next.
                *i -= 1;
            } else {
                state |= 8;
                match fetch(bytes, i) {
                    Some(b']') => break 'scan,
                    Some(x) => {
                        c = x;
                        continue 'scan;
                    }
                    None => return false,
                }
            }
        }

        state |= if c == b':' { 2 } else { 4 };

        match c1 {
            Some(b']') => break 'scan,
            Some(x) => c = x,
            None => return false,
        }
    }

    state == 7
}
