//! Spell checking, using whatever the operating system already provides:
//! NSSpellChecker on macOS and the Windows Spell Checking API on Windows.
//! That way it follows the language and the personal word list the user has
//! already set up, and nothing has to be bundled with the app.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

/// A misspelled run of text, as byte offsets into the string that was checked.
pub type Misspelling = Range<usize>;

/// Results are cached per word: checking happens every frame while typing,
/// and asking the OS about the same word over and over is wasteful.
static CACHE: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

pub fn available() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// Split into word-ish runs, skipping anything that is not prose: URLs,
/// paths, hex blobs and the like are exactly what gets pasted into this app,
/// and flagging them would be noise.
fn words(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();

    // Work line by line, then token by token. A line with anything
    // code-shaped in it - a path, URL, key, hash or command - is skipped
    // whole. Half-checking a command line just flags things like "ssh".
    let mut line_start = 0usize;
    let mut lines: Vec<Range<usize>> = Vec::new();
    for (i, ch) in text.char_indices() {
        if ch == '\n' {
            lines.push(line_start..i);
            line_start = i + 1;
        }
    }
    lines.push(line_start..text.len());

    let code_ish = |t: &str| {
        t.chars().any(|c| c.is_ascii_digit())
            || t.chars().any(|c| matches!(c, '/' | '\\' | '@' | ':' | '_' | '#' | '=' | '~' | '$'))
            || t.chars().filter(|c| c.is_uppercase()).count() > 1
            || t.chars().count() > 30
            // something.like.this, but not an ordinary end of sentence
            || t.trim_end_matches(|c: char| !c.is_alphanumeric()).contains('.')
    };

    let mut tokens: Vec<Range<usize>> = Vec::new();
    for line in lines {
        let mut here: Vec<Range<usize>> = Vec::new();
        let mut token_start = line.start;
        let source = &text[line.clone()];
        for (offset, ch) in source.char_indices() {
            let i = line.start + offset;
            if ch.is_whitespace() {
                if i > token_start {
                    here.push(token_start..i);
                }
                token_start = i + ch.len_utf8();
            }
        }
        if token_start < line.end {
            here.push(token_start..line.end);
        }
        if here.iter().any(|t| code_ish(&text[t.clone()])) {
            continue; // this line is not prose
        }
        tokens.extend(here);
    }

    for token in tokens {
        let t = &text[token.clone()];
        let mut start = None;
        for (i, ch) in t.char_indices() {
            let part_of_word = ch.is_alphabetic() || ch == '\'' || ch == '\u{2019}';
            match (part_of_word, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    out.push(token.start + s..token.start + i);
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            out.push(token.start + s..token.end);
        }
    }

    // Very short words are never worth flagging, and a bare "I'" style
    // fragment is just punctuation.
    out.retain(|r| {
        let w = &text[r.clone()];
        w.chars().filter(|c| c.is_alphabetic()).count() >= 3
    });
    out
}

/// Byte ranges of the words in `text` the system does not recognise.
pub fn check(text: &str) -> Vec<Misspelling> {
    if !available() || text.is_empty() {
        return Vec::new();
    }
    let mut guard = CACHE.lock().unwrap();
    let cache = guard.get_or_insert_with(HashMap::new);
    if cache.len() > 5000 {
        cache.clear();
    }

    let mut bad = Vec::new();
    for range in words(text) {
        let word = &text[range.clone()];
        let known = match cache.get(word) {
            Some(known) => *known,
            None => {
                let known = sys::is_known(word);
                cache.insert(word.to_string(), known);
                known
            }
        };
        if !known {
            bad.push(range);
        }
    }
    bad
}

pub fn suggestions(word: &str) -> Vec<String> {
    if !available() || word.is_empty() {
        return Vec::new();
    }
    sys::suggestions(word)
}

/// Teach the system dictionary a word, so it stops being flagged anywhere.
pub fn learn(word: &str) {
    if available() {
        sys::learn(word);
        if let Some(cache) = CACHE.lock().unwrap().as_mut() {
            cache.insert(word.to_string(), true);
        }
    }
}

// ------------------------------------------------------------------- macOS

#[cfg(target_os = "macos")]
mod sys {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;
    use objc2_foundation::{NSArray, NSRange, NSString};

    // NSSpellChecker lives in AppKit; make sure it is linked in even for
    // targets (like the test harness) that would not otherwise pull it in.
    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    /// The shared checker, or None if AppKit is somehow unavailable - in
    /// which case we simply do not flag anything.
    fn checker() -> Option<*mut AnyObject> {
        let class = AnyClass::get(c"NSSpellChecker")?;
        let shared: *mut AnyObject = unsafe { msg_send![class, sharedSpellChecker] };
        if shared.is_null() {
            return None;
        }
        // Left to identify the language itself, the checker will happily
        // decide a word is Dutch and pass it: "speling" and "wrok" are both
        // real Dutch words. Pin it to the user's own language instead.
        unsafe {
            let _: () = msg_send![shared, setAutomaticallyIdentifiesLanguages: false];
        }
        Some(shared)
    }

    /// The language to check against: whatever the user has chosen for
    /// spelling, falling back to the checker's current one.
    fn language() -> Retained<NSString> {
        static LANG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        let tag = LANG.get_or_init(|| {
            let Some(checker) = checker() else { return "en".into() };
            unsafe {
                let langs: Option<Retained<NSArray<NSString>>> =
                    msg_send![checker, userPreferredLanguages];
                if let Some(first) = langs.and_then(|l| l.iter().next()) {
                    return first.to_string();
                }
                let current: Option<Retained<NSString>> = msg_send![checker, language];
                current.map(|s| s.to_string()).unwrap_or_else(|| "en".into())
            }
        });
        NSString::from_str(tag)
    }

    pub fn is_known(word: &str) -> bool {
        raw_range(word).map(|(_, len)| len == 0).unwrap_or(true)
    }

    /// The range NSSpellChecker reports for the first misspelling, if any.
    pub fn raw_range(word: &str) -> Option<(usize, usize)> {
        let checker = checker()?;
        unsafe {
            let s = NSString::from_str(word);
            let lang = language();
            let mut words: isize = 0;
            let range: NSRange = msg_send![
                checker,
                checkSpellingOfString: &*s,
                startingAt: 0isize,
                language: &*lang,
                wrap: false,
                inSpellDocumentWithTag: 0isize,
                wordCount: &mut words,
            ];
            // NSNotFound (NSIntegerMax) means nothing was wrong with it.
            if range.location >= isize::MAX as usize {
                Some((0, 0))
            } else {
                Some((range.location, range.length))
            }
        }
    }

    pub fn suggestions(word: &str) -> Vec<String> {
        let Some(checker) = checker() else { return Vec::new() };
        unsafe {
            let s = NSString::from_str(word);
            let range = NSRange { location: 0, length: s.len() };
            let lang = language();
            let guesses: Option<Retained<NSArray<NSString>>> = msg_send![
                checker,
                guessesForWordRange: range,
                inString: &*s,
                language: &*lang,
                inSpellDocumentWithTag: 0isize,
            ];
            guesses
                .map(|arr| arr.iter().map(|s| s.to_string()).take(8).collect())
                .unwrap_or_default()
        }
    }

    pub fn learn(word: &str) {
        let Some(checker) = checker() else { return };
        unsafe {
            let s = NSString::from_str(word);
            let _: () = msg_send![checker, learnWord: &*s];
        }
    }
}

// ----------------------------------------------------------------- Windows

#[cfg(target_os = "windows")]
mod sys {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Globalization::{
        ISpellChecker, ISpellCheckerFactory, ISpellingError, SpellCheckerFactory,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };

    use windows::core::PWSTR;
    use windows::Win32::System::Com::CoTaskMemFree;

    thread_local! {
        /// COM objects belong to the thread that made them, and every call
        /// here comes from the UI thread. Making one per word would be slow.
        static CHECKER: std::cell::RefCell<Option<Option<ISpellChecker>>> =
            const { std::cell::RefCell::new(None) };
    }

    fn with_checker<T>(f: impl FnOnce(&ISpellChecker) -> T, default: T) -> T {
        CHECKER.with(|slot| {
            let mut slot = slot.borrow_mut();
            let checker = slot.get_or_insert_with(create_checker);
            match checker {
                Some(c) => f(c),
                None => default,
            }
        })
    }

    fn create_checker() -> Option<ISpellChecker> {
        unsafe {
            // COM is per-thread, so this belongs here rather than behind a
            // one-shot global: a thread that never initialised it gets no
            // spell checker at all. An error here just means the thread was
            // already initialised, which is fine.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let factory: ISpellCheckerFactory =
                CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let tag = HSTRING::from("en-US");
            if factory.IsSupported(PCWSTR(tag.as_ptr())).unwrap_or_default().as_bool() {
                return factory.CreateSpellChecker(PCWSTR(tag.as_ptr())).ok();
            }
            // No en-US installed: take whichever language Windows does have.
            let langs = factory.SupportedLanguages().ok()?;
            let mut buf = [PWSTR::null(); 1];
            let mut fetched = 0u32;
            if langs.Next(&mut buf, Some(&mut fetched)).is_ok() && fetched > 0 {
                let first = buf[0];
                let checker = factory.CreateSpellChecker(PCWSTR(first.0)).ok();
                CoTaskMemFree(Some(first.0 as *const _));
                return checker;
            }
            None
        }
    }

    pub fn is_known(word: &str) -> bool {
        let text = HSTRING::from(word);
        with_checker(
            |checker| unsafe {
                match checker.Check(PCWSTR(text.as_ptr())) {
                    Ok(errors) => {
                        // Next leaves None behind when there is nothing wrong.
                        let mut first: Option<ISpellingError> = None;
                        let _ = errors.Next(&mut first);
                        first.is_none()
                    }
                    Err(_) => true,
                }
            },
            true,
        )
    }

    pub fn suggestions(word: &str) -> Vec<String> {
        let text = HSTRING::from(word);
        with_checker(
            |checker| unsafe {
                let Ok(list) = checker.Suggest(PCWSTR(text.as_ptr())) else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                while out.len() < 8 {
                    let mut buf = [PWSTR::null(); 1];
                    let mut fetched = 0u32;
                    if list.Next(&mut buf, Some(&mut fetched)).is_err() || fetched == 0 {
                        break;
                    }
                    let s = buf[0];
                    if !s.is_null() {
                        if let Ok(text) = s.to_string() {
                            out.push(text);
                        }
                        CoTaskMemFree(Some(s.0 as *const _));
                    }
                }
                out
            },
            Vec::new(),
        )
    }

    pub fn learn(word: &str) {
        let text = HSTRING::from(word);
        with_checker(
            |checker| unsafe {
                let _ = checker.Add(PCWSTR(text.as_ptr()));
            },
            (),
        )
    }
}

// ------------------------------------------------------------------- other

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod sys {
    pub fn is_known(_word: &str) -> bool {
        true
    }
    pub fn suggestions(_word: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn learn(_word: &str) {}
}
