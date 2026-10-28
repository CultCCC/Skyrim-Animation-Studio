//! The UI language. Every user-facing string is written in both languages
//! at its call site: `t!("рус", "eng")` gives a `&'static str`,
//! `tf!("рус {x}", "eng {x}", ...)` a formatted `String`.

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Ru,
    En,
}

pub fn set(lang: Lang) {
    ENGLISH.store(lang == Lang::En, Ordering::Relaxed);
}

pub fn get() -> Lang {
    if en() {
        Lang::En
    } else {
        Lang::Ru
    }
}

#[inline]
pub fn en() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

#[macro_export]
macro_rules! t {
    ($ru:literal, $en:literal) => {
        if $crate::en() { $en } else { $ru }
    };
}

#[macro_export]
macro_rules! tf {
    ($ru:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::en() { format!($en $(, $arg)*) } else { format!($ru $(, $arg)*) }
    };
}

/// A string pair for `const` tables, resolved when shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tr(pub &'static str, pub &'static str);

impl Tr {
    pub fn get(self) -> &'static str {
        if en() {
            self.1
        } else {
            self.0
        }
    }
}

impl std::fmt::Display for Tr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.get())
    }
}

#[macro_export]
macro_rules! tr {
    ($ru:literal, $en:literal) => {
        $crate::Tr($ru, $en)
    };
}
