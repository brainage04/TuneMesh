//! Types for what readers keep of a synced file (contract: "Limits").
//!
//! Every field of a synced JSON file is read into one of these, which cut
//! while deserializing, so a reader keeps a bounded projection and never the
//! parsed file: `Bounded<N>` (text of at most `N` bytes), `BoundedVec<T, N>`
//! (at most `N` items), `TextList<ITEM, N>` (at most `N` texts in one
//! buffer) and `Count<N>` (only the number of items). Structs read from
//! synced files are declared with `bounded_struct!`, which fails to compile
//! when a field's type is not one of these (or a plain number, flag or
//! `Option` of one): a new field cannot skip its limit.

use std::borrow::Borrow;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use serde::de::{self, Deserialize, Deserializer, IgnoredAny, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeSeq, Serializer};

use crate::limits;

/// A type whose deserialized value keeps a bounded amount of memory.
pub trait Limited {}

impl Limited for bool {}
impl Limited for u32 {}
impl Limited for u64 {}
impl Limited for f64 {}
impl<T: Limited> Limited for Option<T> {}

/// Compile-time check used by `bounded_struct!`.
pub const fn assert_limited<T: Limited>() {}

/// Declares a struct read from a synced file. Every field's type must be
/// `Limited`, checked at compile time.
#[macro_export]
macro_rules! bounded_struct {
    (
        $(#[$m:meta])*
        $vis:vis struct $name:ident {
            $( $(#[$fm:meta])* $fvis:vis $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$m])*
        $vis struct $name {
            $( $(#[$fm])* $fvis $field : $ty ),*
        }
        impl $crate::bounded::Limited for $name {}
        const _: () = { $( $crate::bounded::assert_limited::<$ty>(); )* };
    };
}

/// Text of at most `N` bytes, cut at a character boundary when created or
/// deserialized. Remembers whether it was cut (`was_cut`), so a reader can
/// tell a too-long value without keeping it.
#[derive(Clone, Default)]
pub struct Bounded<const N: usize> {
    text: Box<str>,
    cut: bool,
}

impl<const N: usize> Limited for Bounded<N> {}

impl<const N: usize> Bounded<N> {
    /// `s` cut to `N` bytes.
    pub fn new(s: &str) -> Self {
        let kept = limits::cut(s, N);
        Self {
            cut: kept.len() < s.len(),
            text: kept.into(),
        }
    }

    /// `s` cut to `N` bytes, reusing its buffer when it fits.
    pub fn from_string(mut s: String) -> Self {
        let cut = s.len() > N;
        if cut {
            limits::cut_in_place(&mut s, N);
        }
        Self {
            text: s.into_boxed_str(),
            cut,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether the original text was longer than `N` bytes.
    pub fn was_cut(&self) -> bool {
        self.cut
    }
}

impl<const N: usize> Deref for Bounded<N> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl<const N: usize> AsRef<str> for Bounded<N> {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

impl<const N: usize> Borrow<str> for Bounded<N> {
    fn borrow(&self) -> &str {
        &self.text
    }
}

impl<const N: usize> From<&str> for Bounded<N> {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl<const N: usize> From<String> for Bounded<N> {
    fn from(s: String) -> Self {
        Self::from_string(s)
    }
}

impl<const N: usize> fmt::Debug for Bounded<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.text, f)
    }
}

impl<const N: usize> fmt::Display for Bounded<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&*self.text, f)
    }
}

/// Equal when the kept text is equal (whether it was cut does not count).
impl<const N: usize, const M: usize> PartialEq<Bounded<M>> for Bounded<N> {
    fn eq(&self, other: &Bounded<M>) -> bool {
        self.text == other.text
    }
}

impl<const N: usize> Eq for Bounded<N> {}

impl<const N: usize> PartialEq<str> for Bounded<N> {
    fn eq(&self, other: &str) -> bool {
        &*self.text == other
    }
}

impl<const N: usize> PartialEq<&str> for Bounded<N> {
    fn eq(&self, other: &&str) -> bool {
        &*self.text == *other
    }
}

impl<const N: usize> std::hash::Hash for Bounded<N> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.text.hash(state);
    }
}

impl<const N: usize> Serialize for Bounded<N> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.text)
    }
}

impl<'de, const N: usize> Deserialize<'de> for Bounded<N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<const N: usize>;
        impl<const N: usize> Visitor<'_> for V<N> {
            type Value = Bounded<N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
                Ok(Bounded::new(s))
            }
            fn visit_string<E: de::Error>(self, s: String) -> Result<Self::Value, E> {
                // A string the parser built (it had escapes): copy only the
                // kept part, so its full buffer is not kept.
                Ok(Bounded::new(&s))
            }
        }
        d.deserialize_str(V::<N>)
    }
}

/// At most `N` items; more in the file are skipped without being kept, but
/// counted (`total`).
#[derive(Clone)]
pub struct BoundedVec<T, const N: usize> {
    items: Vec<T>,
    total: usize,
}

impl<T: Limited, const N: usize> Limited for BoundedVec<T, N> {}

impl<T, const N: usize> Default for BoundedVec<T, N> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            total: 0,
        }
    }
}

impl<T, const N: usize> BoundedVec<T, N> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `item` if fewer than `N` are kept; returns whether it was kept.
    /// Counted in `total` either way.
    pub fn push(&mut self, item: T) -> bool {
        self.total = self.total.saturating_add(1);
        if self.items.len() < N {
            self.items.push(item);
            true
        } else {
            false
        }
    }

    /// Removes the last kept item (and its count).
    pub fn pop(&mut self) -> Option<T> {
        let item = self.items.pop()?;
        self.total -= 1;
        Some(item)
    }

    /// Items the file had, kept or not.
    pub fn total(&self) -> usize {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<T, const N: usize> Deref for BoundedVec<T, N> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.items
    }
}

impl<T, const N: usize> DerefMut for BoundedVec<T, N> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.items
    }
}

impl<T, const N: usize> FromIterator<T> for BoundedVec<T, N> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut v = Self::new();
        for item in iter {
            v.push(item);
        }
        v.items.shrink_to_fit();
        v
    }
}

impl<T: fmt::Debug, const N: usize> fmt::Debug for BoundedVec<T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.items).finish()
    }
}

/// Equal when the kept items are equal.
impl<T: PartialEq, const N: usize> PartialEq for BoundedVec<T, N> {
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
    }
}

impl<T: Serialize, const N: usize> Serialize for BoundedVec<T, N> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.items.len()))?;
        for item in &self.items {
            seq.serialize_element(item)?;
        }
        seq.end()
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for BoundedVec<T, N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T, const N: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for V<T, N> {
            type Value = BoundedVec<T, N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut v = BoundedVec::new();
                while v.items.len() < N {
                    match seq.next_element()? {
                        Some(item) => _ = v.push(item),
                        None => break,
                    }
                }
                while seq.next_element::<IgnoredAny>()?.is_some() {
                    v.total = v.total.saturating_add(1);
                }
                v.items.shrink_to_fit();
                Ok(v)
            }
        }
        d.deserialize_seq(V::<T, N>(PhantomData))
    }
}

/// At most `N` texts of at most `ITEM` bytes each, in one buffer (4 bytes
/// per text besides the text itself), so a list of many short paths costs
/// little more than its file. More texts are skipped and counted (`total`).
#[derive(Clone, Default)]
pub struct TextList<const ITEM: usize, const N: usize> {
    text: String,
    ends: Vec<u32>,
    total: usize,
}

impl<const ITEM: usize, const N: usize> Limited for TextList<ITEM, N> {}

impl<const ITEM: usize, const N: usize> TextList<ITEM, N> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `s`, cut to `ITEM` bytes, if fewer than `N` are kept; returns
    /// whether it was kept. Counted in `total` either way.
    pub fn push(&mut self, s: &str) -> bool {
        // Offsets are u32.
        const { assert!(ITEM.saturating_mul(N) <= u32::MAX as usize) };
        self.total = self.total.saturating_add(1);
        if self.ends.len() >= N {
            return false;
        }
        self.text.push_str(limits::cut(s, ITEM));
        self.ends.push(self.text.len() as u32);
        true
    }

    /// Removes the last kept text (and its count).
    pub fn pop(&mut self) -> Option<String> {
        self.ends.pop()?;
        self.total -= 1;
        let start = self.ends.last().map_or(0, |e| *e as usize);
        Some(self.text.split_off(start))
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// Texts the file had, kept or not.
    pub fn total(&self) -> usize {
        self.total
    }

    pub fn get(&self, i: usize) -> Option<&str> {
        let end = *self.ends.get(i)? as usize;
        let start = i.checked_sub(1).map_or(0, |p| self.ends[p] as usize);
        Some(&self.text[start..end])
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &str> + DoubleEndedIterator {
        (0..self.ends.len()).map(|i| self.get(i).unwrap_or_default())
    }

    fn shrink_to_fit(&mut self) {
        self.text.shrink_to_fit();
        self.ends.shrink_to_fit();
    }
}

impl<'a, const ITEM: usize, const N: usize> FromIterator<&'a str> for TextList<ITEM, N> {
    fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> Self {
        let mut v = Self::new();
        for s in iter {
            v.push(s);
        }
        v.shrink_to_fit();
        v
    }
}

impl<const ITEM: usize, const N: usize> fmt::Debug for TextList<ITEM, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// Equal when the kept texts are equal.
impl<const ITEM: usize, const N: usize> PartialEq for TextList<ITEM, N> {
    fn eq(&self, other: &Self) -> bool {
        self.ends == other.ends && self.text == other.text
    }
}

impl<const ITEM: usize, const N: usize> Serialize for TextList<ITEM, N> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.len()))?;
        for t in self.iter() {
            seq.serialize_element(t)?;
        }
        seq.end()
    }
}

/// One element of a `TextList` while deserializing: appended straight to
/// the list's buffer, never kept on its own.
struct Append<'l, const ITEM: usize, const N: usize>(&'l mut TextList<ITEM, N>);

impl<'de, const ITEM: usize, const N: usize> de::DeserializeSeed<'de> for Append<'_, ITEM, N> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_str(self)
    }
}

impl<const ITEM: usize, const N: usize> Visitor<'_> for Append<'_, ITEM, N> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string")
    }
    fn visit_str<E: de::Error>(self, s: &str) -> Result<(), E> {
        self.0.push(s);
        Ok(())
    }
}

impl<'de, const ITEM: usize, const N: usize> Deserialize<'de> for TextList<ITEM, N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<const ITEM: usize, const N: usize>;
        impl<'de, const ITEM: usize, const N: usize> Visitor<'de> for V<ITEM, N> {
            type Value = TextList<ITEM, N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list of strings")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut v = TextList::new();
                while v.len() < N {
                    if seq.next_element_seed(Append(&mut v))?.is_none() {
                        break;
                    }
                }
                while seq.next_element::<IgnoredAny>()?.is_some() {
                    v.total = v.total.saturating_add(1);
                }
                v.shrink_to_fit();
                Ok(v)
            }
        }
        d.deserialize_seq(V::<ITEM, N>)
    }
}

/// Only the number of items of a list (at most `N`); the items are skipped
/// without being kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Count<const N: usize>(usize);

impl<const N: usize> Limited for Count<N> {}

impl<const N: usize> Count<N> {
    pub fn get(self) -> usize {
        self.0
    }
}

impl<'de, const N: usize> Deserialize<'de> for Count<N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<const N: usize>;
        impl<'de, const N: usize> Visitor<'de> for V<N> {
            type Value = Count<N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut n = 0usize;
                while seq.next_element::<IgnoredAny>()?.is_some() {
                    n = n.saturating_add(1);
                }
                Ok(Count(n.min(N)))
            }
        }
        d.deserialize_seq(V::<N>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_cuts_and_remembers() {
        let b: Bounded<4> = serde_json::from_str(r#""héllo""#).unwrap();
        assert_eq!(b, "hél");
        assert!(b.was_cut());
        let b: Bounded<4> = serde_json::from_str(r#""a\"b""#).unwrap();
        assert_eq!(b, "a\"b");
        assert!(!b.was_cut());
        assert!(serde_json::from_str::<Bounded<4>>("1").is_err());
        assert_eq!(
            serde_json::to_string(&Bounded::<4>::new("abcdef")).unwrap(),
            r#""abcd""#
        );
    }

    #[test]
    fn lists_keep_n_and_count_all() {
        let v: BoundedVec<Bounded<2>, 2> = serde_json::from_str(r#"["aaa","b",[1],"c"]"#).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v.total(), 4);
        assert_eq!(&*v[0], "aa");
        // Items past N are skipped whatever they are.
        let t: TextList<3, 2> = serde_json::from_str(r#"["abcd","\u00e9\u00e9",{},"x"]"#).unwrap();
        assert_eq!(t.iter().collect::<Vec<_>>(), ["abc", "é"]);
        assert_eq!(t.total(), 4);
        let c: Count<3> = serde_json::from_str(r#"[1,"2",[3],4,5]"#).unwrap();
        assert_eq!(c.get(), 3);
        // Kept items are checked like any other.
        assert!(serde_json::from_str::<TextList<3, 2>>("[1]").is_err());
        let mut t2 = t.clone();
        assert_eq!(t2.pop().as_deref(), Some("é"));
        assert_eq!(t2.iter().collect::<Vec<_>>(), ["abc"]);
        assert_eq!(serde_json::to_string(&t).unwrap(), r#"["abc","é"]"#);
    }
}
