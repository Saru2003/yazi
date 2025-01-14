use std::{cmp::Ordering, collections::HashMap, mem};

use yazi_config::manager::SortBy;
use yazi_shared::{LcgRng, fs::{File, UrnBuf}, natsort, translit::Transliterator};

#[derive(Clone, Copy, Default, PartialEq)]
pub struct FilesSorter {
	pub by:        SortBy,
	pub sensitive: bool,
	pub reverse:   bool,
	pub dir_first: bool,
	pub translit:  bool,
}

impl FilesSorter {
	pub(super) fn sort(&self, items: &mut Vec<File>, sizes: &HashMap<UrnBuf, u64>) {
		if items.is_empty() {
			return;
		}

		let by_alphabetical = |a: &File, b: &File| {
			if self.sensitive {
				self.cmp(a.name(), b.name(), self.promote(a, b))
			} else {
				self.cmp(a.name().to_ascii_uppercase(), b.name().to_ascii_uppercase(), self.promote(a, b))
			}
		};

		match self.by {
			SortBy::None => {}
			SortBy::Mtime => items.sort_unstable_by(|a, b| {
				let ord = self.cmp(a.mtime, b.mtime, self.promote(a, b));
				if ord == Ordering::Equal { by_alphabetical(a, b) } else { ord }
			}),
			SortBy::Btime => items.sort_unstable_by(|a, b| {
				let ord = self.cmp(a.btime, b.btime, self.promote(a, b));
				if ord == Ordering::Equal { by_alphabetical(a, b) } else { ord }
			}),
			SortBy::Extension => items.sort_unstable_by(|a, b| {
				let ord = if self.sensitive {
					self.cmp(a.url.extension(), b.url.extension(), self.promote(a, b))
				} else {
					self.cmp(
						a.url.extension().map(|s| s.to_ascii_lowercase()),
						b.url.extension().map(|s| s.to_ascii_lowercase()),
						self.promote(a, b),
					)
				};
				if ord == Ordering::Equal { by_alphabetical(a, b) } else { ord }
			}),
			SortBy::Alphabetical => items.sort_unstable_by(by_alphabetical),
			SortBy::Natural => self.sort_naturally(items),
			SortBy::Size => items.sort_unstable_by(|a, b| {
				let aa = if a.is_dir() { sizes.get(a.urn()).copied() } else { None };
				let bb = if b.is_dir() { sizes.get(b.urn()).copied() } else { None };
				let ord = self.cmp(aa.unwrap_or(a.len), bb.unwrap_or(b.len), self.promote(a, b));
				if ord == Ordering::Equal { by_alphabetical(a, b) } else { ord }
			}),
			SortBy::Random => {
				let mut rng = LcgRng::default();
				items.sort_unstable_by(|a, b| self.cmp(rng.next(), rng.next(), self.promote(a, b)))
			}
		}
	}

	fn sort_naturally(&self, items: &mut Vec<File>) {
		let mut indices: Vec<usize> = (0..items.len()).collect();
		indices.sort_unstable_by(|&a, &b| {
			let (a, b) = (&items[a], &items[b]);

			let promote = self.promote(a, b);
			if promote != Ordering::Equal {
				return promote;
			}

			let ordering = if self.translit {
				natsort(
					a.name().as_encoded_bytes().transliterate().as_bytes(),
					b.name().as_encoded_bytes().transliterate().as_bytes(),
					!self.sensitive,
				)
			} else {
				natsort(a.name().as_encoded_bytes(), b.name().as_encoded_bytes(), !self.sensitive)
			};

			if self.reverse { ordering.reverse() } else { ordering }
		});

		*items = indices.into_iter().map(|i| mem::take(&mut items[i])).collect();
	}

	#[inline(always)]
	#[allow(clippy::collapsible_else_if)]
	fn cmp<T: Ord>(&self, a: T, b: T, promote: Ordering) -> Ordering {
		if promote != Ordering::Equal {
			promote
		} else {
			if self.reverse { b.cmp(&a) } else { a.cmp(&b) }
		}
	}

	#[inline(always)]
	fn promote(&self, a: &File, b: &File) -> Ordering {
		if self.dir_first { b.is_dir().cmp(&a.is_dir()) } else { Ordering::Equal }
	}
}
