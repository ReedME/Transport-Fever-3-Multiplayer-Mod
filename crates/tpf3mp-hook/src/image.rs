//! Reading the running game's memory safely: whether an address may be read
//! before the hook reads it on the game's thread (`crate::order`,
//! `crate::seeds`).
//!
//! Windows only for now, like the step gate. Elsewhere nothing is readable,
//! and the hook's native reads refuse (fail closed).
//!
//! [`readable`] asks the system every time (`VirtualQuery`, a system call:
//! about a microsecond, tens of microseconds inside Sandboxie, which hooks
//! system calls). The hot paths check and read through [`Readable`]
//! instead, which reads through [`guarded`]: no system call, a fault on a
//! misread address refused by the hook's vectored handler. With guarded
//! reads off (`TPF3MP_HOOK_GUARDED_READS=0`) it checks with `VirtualQuery`
//! through a per-thread cache that remembers the regions found readable
//! until [`invalidate`], which the hook calls before every simulation
//! update and at every world change.

#![allow(unsafe_code)]

pub mod guarded;

use std::{
    cell::RefCell,
    sync::atomic::{AtomicU64, Ordering},
};

/// The committed, readable region `[base, end)` that holds `address`, or
/// `None` when `address` is not readable.
#[cfg(windows)]
fn query(address: usize) -> Option<(usize, usize)> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery,
    };
    // SAFETY: MEMORY_BASIC_INFORMATION is plain data, zero is a valid value
    // for it, and VirtualQuery only writes into it.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let written = unsafe {
        VirtualQuery(
            address as *const _,
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if written == 0 || info.State != MEM_COMMIT || info.Protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
    {
        return None;
    }
    let base = info.BaseAddress as usize;
    Some((base, base.saturating_add(info.RegionSize)))
}

/// Nothing is known readable on this platform.
#[cfg(not(windows))]
fn query(_address: usize) -> Option<(usize, usize)> {
    None
}

/// The committed, readable region holding `address`, as the system answers
/// it now (one `VirtualQuery`), for a cache of the caller's own that lives
/// no longer than the structures it checks ([`crate::netread`]).
pub fn region(address: usize) -> Option<(usize, usize)> {
    query(address)
}

/// Regions already found readable, most recent first; at most `N`.
#[derive(Debug, Clone)]
pub struct RegionCache<const N: usize> {
    regions: [(usize, usize); N],
    len: usize,
}

impl<const N: usize> RegionCache<N> {
    pub const fn new() -> Self {
        Self {
            regions: [(0, 0); N],
            len: 0,
        }
    }

    /// Whether `len` bytes at `address` are readable, asking `query` only
    /// for the parts no remembered region covers. `query(at)` answers the
    /// readable region `[base, end)` holding `at`, or `None`.
    pub fn readable_with(
        &mut self,
        address: usize,
        len: usize,
        mut query: impl FnMut(usize) -> Option<(usize, usize)>,
    ) -> bool {
        if len == 0 {
            return true;
        }
        let Some(end) = address.checked_add(len) else {
            return false;
        };
        let mut at = address;
        while at < end {
            if let Some(i) = self.regions[..self.len]
                .iter()
                .position(|(base, region_end)| *base <= at && at < *region_end)
            {
                let region_end = self.regions[i].1;
                // A hit moves to the front, so the regions in use stay
                // and the least recently used one goes first: evicting by
                // age alone thrashed once more than N regions were in play.
                self.regions[..=i].rotate_right(1);
                at = region_end;
                continue;
            }
            let Some((base, region_end)) = query(at) else {
                return false;
            };
            if base > at || region_end <= at {
                return false;
            }
            self.remember(base, region_end);
            at = region_end;
        }
        true
    }

    /// Puts `[base, end)` first, merged with every remembered region it
    /// overlaps or touches. VirtualQuery answers a region from the page of
    /// the address asked, not from where the region begins, so a check below
    /// a remembered region's start asks again and gets an overlapping answer:
    /// merged, it is one entry, not a second copy crowding out the others.
    fn remember(&mut self, mut base: usize, mut end: usize) {
        if N == 0 {
            return;
        }
        let mut kept = 0;
        for i in 0..self.len {
            let (other_base, other_end) = self.regions[i];
            if other_base <= end && base <= other_end {
                base = base.min(other_base);
                end = end.max(other_end);
            } else {
                self.regions[kept] = self.regions[i];
                kept += 1;
            }
        }
        let keep = kept.min(N - 1);
        self.regions.copy_within(0..keep, 1);
        self.regions[0] = (base, end);
        self.len = keep + 1;
    }

    /// How many regions are remembered.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> Default for RegionCache<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// A [`RegionCache`] that is good for one epoch: a check in a later epoch
/// forgets every region first.
#[derive(Debug, Clone)]
pub struct EpochCache<const N: usize> {
    epoch: u64,
    cache: RegionCache<N>,
}

impl<const N: usize> EpochCache<N> {
    pub const fn new() -> Self {
        Self {
            epoch: 0,
            cache: RegionCache::new(),
        }
    }

    /// As [`RegionCache::readable_with`], in `epoch`; also answers how many
    /// regions `query` was asked for (0: answered from the cache).
    pub fn readable_with(
        &mut self,
        epoch: u64,
        address: usize,
        len: usize,
        mut query: impl FnMut(usize) -> Option<(usize, usize)>,
    ) -> (bool, u32) {
        if epoch != self.epoch {
            self.epoch = epoch;
            self.cache = RegionCache::new();
        }
        let mut asked = 0;
        let readable = self.cache.readable_with(address, len, |at| {
            asked += 1;
            query(at)
        });
        (readable, asked)
    }
}

impl<const N: usize> Default for EpochCache<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Regions found readable this epoch by any thread, sorted by base and
/// never overlapping: the level behind each thread's [`RegionCache`]. The
/// hook's reads run on up to all of the game's threads, and each thread's
/// own cache is dropped every update, so without it every thread asked the
/// system again for the regions another had just asked about (about 4.5
/// system calls per road-fix append, on every map size).
#[derive(Debug, Default)]
pub struct SharedRegions {
    epoch: u64,
    regions: Vec<(usize, usize)>,
}

impl SharedRegions {
    /// Regions kept at most; past it the list starts over (a cost, never a
    /// wrong answer).
    pub const MAX: usize = 1 << 16;

    pub const fn new() -> Self {
        Self {
            epoch: 0,
            regions: Vec::new(),
        }
    }

    /// The remembered region holding `at` in `epoch`, if any.
    pub fn lookup(&self, epoch: u64, at: usize) -> Option<(usize, usize)> {
        if epoch != self.epoch {
            return None;
        }
        let i = self.regions.partition_point(|&(base, _)| base <= at);
        let (base, end) = *self.regions.get(i.checked_sub(1)?)?;
        (base <= at && at < end).then_some((base, end))
    }

    /// Remembers `[base, end)` for `epoch`: a later epoch forgets the
    /// rest first, an earlier one (a check that began before an
    /// invalidation) is not kept. Overlapping or touching regions merge.
    pub fn insert(&mut self, epoch: u64, base: usize, end: usize) {
        if epoch < self.epoch || base >= end {
            return;
        }
        if epoch > self.epoch || self.regions.len() >= Self::MAX {
            self.epoch = epoch;
            self.regions.clear();
        }
        let first = self.regions.partition_point(|&(_, e)| e < base);
        let last = self.regions.partition_point(|&(b, _)| b <= end);
        let (mut base, mut end) = (base, end);
        if first < last {
            base = base.min(self.regions[first].0);
            end = end.max(self.regions[last - 1].1);
        }
        self.regions
            .splice(first..last, std::iter::once((base, end)));
    }

    /// How many regions are remembered.
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

static SHARED_REGIONS: std::sync::RwLock<SharedRegions> =
    std::sync::RwLock::new(SharedRegions::new());

thread_local! {
    /// The system calls this thread's last check made.
    static ASKED_SYSTEM: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The region holding `at`: from the shared level if another thread
/// already asked this epoch, else from the system, then shared.
fn shared_query(epoch: u64, at: usize) -> Option<(usize, usize)> {
    if let Ok(shared) = SHARED_REGIONS.read()
        && let Some(region) = shared.lookup(epoch, at)
    {
        return Some(region);
    }
    ASKED_SYSTEM.with(|asked| asked.set(asked.get() + 1));
    let region = query(at)?;
    if let Ok(mut shared) = SHARED_REGIONS.write() {
        shared.insert(epoch, region.0, region.1);
    }
    Some(region)
}

/// The epoch of every thread's [`Readable`] cache. Starts at 1, so a fresh
/// cache (epoch 0) is always refilled.
static EPOCH: AtomicU64 = AtomicU64::new(1);
/// Checks answered from a cache, and checks that asked the system.
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// Regions each thread's cache remembers. 8 was enough on stock maps; a
/// big map's world spreads over far more heap regions, and with 8 the road
/// fix's walk evicted regions it needed again within the same update: on a
/// 100 x 1000 tile world a fifth of all checks (230,000 per 10 s) asked
/// `VirtualQuery`, 12% of the simulation thread's working time. A hit is a
/// short scan, a miss a system call, so the list can be longer.
pub const REGIONS_PER_THREAD: usize = 32;

thread_local! {
    static SHARED: RefCell<EpochCache<REGIONS_PER_THREAD>> = const { RefCell::new(EpochCache::new()) };
}

/// Forgets every region every thread's [`Readable`] cache remembers (they
/// refill on their next check). Called when memory may have been freed
/// that a cached region covered: before each simulation update and each
/// call of the game's step, when a world's GUI starts, when a load is asked
/// for.
pub fn invalidate() {
    EPOCH.fetch_add(1, Ordering::AcqRel);
}

/// Guarded reads and those a fault refused since the last take (the
/// `perf:` line's).
pub fn take_guarded_counts() -> (u64, u64) {
    guarded::take_counts()
}

/// The cache's hits and misses since the last take (the `perf:` line's).
pub fn take_counts() -> (u64, u64) {
    (
        HITS.swap(0, Ordering::Relaxed),
        MISSES.swap(0, Ordering::Relaxed),
    )
}

/// Readability checks and reads on the hot paths (the road fix's walk, the
/// order fixes' vectors, the game scripts' reseed).
///
/// With guarded reads on ([`guarded::active`], the default) a check reads
/// one byte of every page the range spans and a read copies the value out,
/// both through [`guarded`]'s routines: a misread address faults, and the
/// hook's vectored handler turns the fault into a refusal. No system call,
/// no cache.
///
/// Off, checks go through this thread's region cache: a region the system
/// said was committed and readable is remembered until the next
/// [`invalidate`], so `VirtualQuery` is asked once per region and update,
/// not once per word. Inside Sandboxie a `VirtualQuery` costs tens of
/// microseconds, so this was most of the hook's cost there.
///
/// A check, either way, says the memory could be read at that moment;
/// reading it later through a raw pointer, or writing it (a successful
/// check proves nothing about writing; neither did `VirtualQuery`'s), is
/// safe as long as nothing the engine frees in between is touched: every
/// address the hook reads comes from a structure the engine keeps live,
/// the checks only guard against a layout the hook misreads, and the cache
/// is dropped at every update and world change, where the engine frees.
#[derive(Debug, Clone, Copy, Default)]
pub struct Readable;

impl Readable {
    pub const fn new() -> Self {
        Self
    }

    /// Whether `len` bytes at `address` are committed, readable memory.
    pub fn readable(&mut self, address: usize, len: usize) -> bool {
        if cfg!(not(windows)) {
            return false;
        }
        if guarded::active() {
            return guarded::probe(address, len);
        }
        readable_by_query(address, len)
    }

    /// A plain value of the game's memory, only if it is readable. `T` is
    /// plain data: every bit pattern a valid value.
    pub fn read<T: Copy>(&mut self, address: u64) -> Option<T> {
        let address = usize::try_from(address).ok()?;
        if cfg!(windows) && guarded::active() {
            return guarded::read(address);
        }
        if !readable_by_query(address, std::mem::size_of::<T>()) {
            return None;
        }
        // SAFETY: `size_of::<T>()` bytes at `address` are committed,
        // readable memory, checked just above in this epoch; the read is
        // unaligned and by value.
        Some(unsafe { std::ptr::read_unaligned(address as *const T) })
    }
}

/// [`Readable::readable`] with guarded reads off: through this thread's
/// region cache and the shared level, asking `VirtualQuery` for what
/// neither knows.
fn readable_by_query(address: usize, len: usize) -> bool {
    if cfg!(not(windows)) {
        return false;
    }
    let epoch = EPOCH.load(Ordering::Acquire);
    ASKED_SYSTEM.with(|asked| asked.set(0));
    let (readable, _) = SHARED.with(|cache| {
        cache
            .borrow_mut()
            .readable_with(epoch, address, len, |at| shared_query(epoch, at))
    });
    if ASKED_SYSTEM.with(std::cell::Cell::get) == 0 {
        HITS.fetch_add(1, Ordering::Relaxed);
    } else {
        MISSES.fetch_add(1, Ordering::Relaxed);
    }
    readable
}

/// [`Readable::readable`], for one check.
pub fn readable_cached(address: usize, len: usize) -> bool {
    Readable.readable(address, len)
}

/// Whether `len` bytes at `address` are committed, readable memory: a fresh
/// question to the system every time.
#[cfg(windows)]
pub fn readable(address: usize, len: usize) -> bool {
    RegionCache::<0>::new().readable_with(address, len, query)
}

/// Whether `len` bytes at `address` may be read. Not known here, so `false`:
/// nothing native is read on this platform yet.
#[cfg(not(windows))]
pub fn readable(_address: usize, _len: usize) -> bool {
    false
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn the_shared_level_finds_merges_and_forgets_by_epoch() {
        let mut shared = SharedRegions::new();
        shared.insert(1, 0x3000, 0x4000);
        shared.insert(1, 0x1000, 0x2000);
        assert_eq!(shared.lookup(1, 0x1800), Some((0x1000, 0x2000)));
        assert_eq!(shared.lookup(1, 0x3fff), Some((0x3000, 0x4000)));
        assert_eq!(shared.lookup(1, 0x2000), None, "between regions");
        assert_eq!(shared.lookup(1, 0x0fff), None, "below every region");
        // Touching and overlapping answers merge into one.
        shared.insert(1, 0x2000, 0x3000);
        assert_eq!(shared.len(), 1);
        assert_eq!(shared.lookup(1, 0x2800), Some((0x1000, 0x4000)));
        // Another epoch's question gets nothing; an older epoch's answer is
        // not kept; a newer one starts over.
        assert_eq!(shared.lookup(2, 0x1800), None);
        shared.insert(0, 0x9000, 0xa000);
        assert_eq!(shared.lookup(1, 0x9800), None);
        shared.insert(2, 0x9000, 0xa000);
        assert_eq!(shared.len(), 1);
        assert_eq!(shared.lookup(2, 0x1800), None, "forgotten with its epoch");
        assert_eq!(shared.lookup(2, 0x9800), Some((0x9000, 0xa000)));
    }

    #[test]
    fn the_shared_level_agrees_with_a_plain_list_on_random_regions() {
        // Disjoint random regions, inserted in random order, looked up at
        // random points: the same answer as a linear search.
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut plain = Vec::new();
        let mut at = 0x10000usize;
        for _ in 0..500 {
            at += (next() % 0x8000) as usize + 1;
            let len = (next() % 0x8000) as usize + 1;
            plain.push((at, at + len));
            at += len;
        }
        let mut shared = SharedRegions::new();
        let mut order: Vec<usize> = (0..plain.len()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, (next() % (i as u64 + 1)) as usize);
        }
        for &i in &order {
            shared.insert(7, plain[i].0, plain[i].1);
        }
        for _ in 0..20_000 {
            let probe = 0x10000 + (next() % (at as u64)) as usize;
            let expected = plain
                .iter()
                .find(|(b, e)| *b <= probe && probe < *e)
                .copied();
            let got = shared.lookup(7, probe);
            // Regions that touch merge, so compare containment.
            match (expected, got) {
                (None, None) => {}
                (Some(_), Some((b, e))) => assert!(b <= probe && probe < e),
                other => panic!("{probe:#x}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_walk_over_twenty_regions_asks_once_per_region() {
        // A big map's road walk touches many heap regions each update; the
        // shared cache must hold them all, not thrash.
        let mut cache = EpochCache::<REGIONS_PER_THREAD>::new();
        let mut asked = 0;
        for _round in 0..5 {
            for region in 0..20usize {
                let base = 0x10_0000 * (region + 1);
                let (readable, n) = cache.readable_with(1, base + 8, 8, |at| {
                    Some((at & !0xF_FFFF, (at & !0xF_FFFF) + 0x1000))
                });
                assert!(readable);
                asked += n;
            }
        }
        assert_eq!(asked, 20);
    }

    #[test]
    fn a_remembered_region_answers_without_asking_again() {
        let mut cache = RegionCache::<2>::new();
        let asked = std::cell::RefCell::new(Vec::new());
        let mut query = |at: usize| {
            asked.borrow_mut().push(at);
            // Readable: [0x1000, 0x3000) and [0x3000, 0x4000); not below.
            match at {
                0x1000..0x3000 => Some((0x1000, 0x3000)),
                0x3000..0x4000 => Some((0x3000, 0x4000)),
                _ => None,
            }
        };
        assert!(cache.readable_with(0x1010, 8, &mut query));
        assert!(cache.readable_with(0x2ff0, 0x10, &mut query));
        assert!(cache.readable_with(0x1000, 0x2000, &mut query));
        assert_eq!(*asked.borrow(), vec![0x1010], "one question for the region");
        // Across the region's end: the next region is asked for once.
        assert!(cache.readable_with(0x2ff8, 0x10, &mut query));
        assert!(cache.readable_with(0x3008, 8, &mut query));
        assert_eq!(*asked.borrow(), vec![0x1010, 0x3000]);
        // The two touch, and both are readable: one entry.
        assert_eq!(cache.len(), 1);
        // Past the readable memory: refused, whatever is remembered.
        assert!(!cache.readable_with(0x3ff8, 0x10, &mut query));
        assert!(!cache.readable_with(0x800, 8, &mut query));
        assert!(!cache.readable_with(usize::MAX - 4, 8, &mut query));
        assert!(cache.readable_with(0x10, 0, &mut query), "zero bytes");
    }

    /// Separate one-page regions (a gap between each, so none merge).
    fn pages(asked: &std::cell::Cell<usize>) -> impl FnMut(usize) -> Option<(usize, usize)> + '_ {
        move |at: usize| {
            asked.set(asked.get() + 1);
            let base = at & !0xfff;
            Some((base, base + 0x1000))
        }
    }

    #[test]
    fn the_least_recently_used_region_is_forgotten_first() {
        let mut cache = RegionCache::<2>::new();
        let asked = std::cell::Cell::new(0);
        let mut query = pages(&asked);
        for page in [0x1000, 0x3000, 0x5000] {
            assert!(cache.readable_with(page, 4, &mut query));
        }
        assert_eq!(cache.len(), 2);
        // 0x3000 and 0x5000 remembered; 0x1000 was forgotten.
        assert!(cache.readable_with(0x5004, 4, &mut query));
        assert!(cache.readable_with(0x3004, 4, &mut query));
        assert_eq!(asked.get(), 3);
        assert!(cache.readable_with(0x1004, 4, &mut query));
        assert_eq!(asked.get(), 4);
    }

    #[test]
    fn a_region_in_use_stays_while_others_come_and_go() {
        // One region read between visits to many others: evicting by age
        // alone asked for it again each time (the thrashing seen in the
        // road-entry benchmark); a hit keeps it.
        let mut cache = RegionCache::<2>::new();
        let asked = std::cell::Cell::new(0);
        let mut query = pages(&asked);
        assert!(cache.readable_with(0x1000, 4, &mut query));
        for other in [0x3000, 0x5000, 0x7000, 0x9000] {
            assert!(cache.readable_with(other, 4, &mut query));
            assert!(cache.readable_with(0x1004, 4, &mut query));
        }
        assert_eq!(asked.get(), 5, "0x1000 asked once, each other once");
    }

    #[test]
    fn a_check_below_a_remembered_start_merges_into_one_region() {
        // VirtualQuery answers from the page asked: [page, region end).
        let mut cache = RegionCache::<4>::new();
        let asked = std::cell::Cell::new(0);
        let mut query = |at: usize| {
            asked.set(asked.get() + 1);
            Some((at & !0xfff, 0x10_000))
        };
        assert!(cache.readable_with(0x8000, 4, &mut query));
        assert!(cache.readable_with(0x2000, 4, &mut query));
        assert_eq!(cache.len(), 1, "one region, not two overlapping copies");
        assert!(cache.readable_with(0x5000, 4, &mut query));
        assert!(cache.readable_with(0x9000, 4, &mut query));
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn a_cache_of_a_past_epoch_refuses_what_was_freed_since() {
        let mut cache = EpochCache::<4>::new();
        let mapped = std::cell::Cell::new(true);
        let query = |at: usize| {
            (mapped.get() && (0x1000..0x2000).contains(&at)).then_some((0x1000, 0x2000))
        };
        assert_eq!(cache.readable_with(1, 0x1800, 8, query), (true, 1));
        assert_eq!(cache.readable_with(1, 0x1808, 8, query), (true, 0), "a hit");
        // The world goes and its memory with it.
        mapped.set(false);
        assert_eq!(
            cache.readable_with(1, 0x1800, 8, query),
            (true, 0),
            "within the epoch the cache still answers: why every world change invalidates"
        );
        assert_eq!(
            cache.readable_with(2, 0x1800, 8, query),
            (false, 1),
            "the next epoch asks again, and the freed address is refused"
        );
    }

    #[test]
    fn a_region_that_does_not_hold_the_address_is_refused() {
        let mut cache = RegionCache::<4>::new();
        assert!(!cache.readable_with(0x1000, 4, |_| Some((0x2000, 0x3000))));
        assert!(!cache.readable_with(0x1000, 4, |_| Some((0x800, 0x1000))));
        assert!(cache.is_empty());
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn this_code_is_readable_and_unmapped_memory_is_not() {
        let here = this_code_is_readable_and_unmapped_memory_is_not as *const () as usize;
        assert!(readable(here, 16));
        // The null page is never mapped.
        assert!(!readable(0x10, 8));
        assert!(!readable(usize::MAX - 4, 8));
        assert!(readable(0x10, 0), "zero bytes are always readable");
        let mut probe = Readable::new();
        let value = 0x1234_5678_u32;
        assert_eq!(
            probe.read::<u32>(&value as *const u32 as u64),
            Some(0x1234_5678)
        );
        assert_eq!(probe.read::<u32>(0x10), None);
        assert!(readable_cached(here, 16));
    }

    #[test]
    fn a_freed_page_is_refused_once_the_cache_is_invalidated() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        };
        // SAFETY: a fresh page of our own.
        let page = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                0x1000,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        } as usize;
        assert_ne!(page, 0);
        assert!(readable_cached(page, 8));
        assert!(readable_cached(page + 8, 8));
        // SAFETY: the page is ours and nothing refers to it any more.
        assert_ne!(unsafe { VirtualFree(page as *mut _, 0, MEM_RELEASE) }, 0);
        invalidate();
        assert!(!readable_cached(page, 8), "freed, and the cache forgot it");
        assert_eq!(Readable::new().read::<u64>(page as u64), None);
    }

    #[test]
    fn the_query_path_still_answers_as_before() {
        // The path the kill switch falls back to.
        let value = 7u64;
        let at = &value as *const u64 as usize;
        invalidate();
        assert!(readable_by_query(at, 8));
        assert!(!readable_by_query(0x10, 8));
        assert!(!readable_by_query(usize::MAX - 4, 8));
    }

    /// Old against new: a check and a read through the `VirtualQuery`
    /// cache, through guarded reads, and a fresh `VirtualQuery`. Run with
    /// `cargo test --release -p tpf3mp-hook image::tests::readable_bench -- --ignored --nocapture`.
    #[test]
    #[ignore = "a benchmark: prints a readability check's and a read's cost each way"]
    fn readable_bench() {
        let words: Vec<u64> = (0..64).collect();
        let at = words.as_ptr() as usize;
        const N: usize = 1_000_000;
        let time = |f: &mut dyn FnMut(usize)| {
            let begin = std::time::Instant::now();
            for i in 0..N {
                f(i);
            }
            begin.elapsed().as_nanos() as f64 / N as f64
        };
        let fresh = time(&mut |i| assert!(readable(at + 8 * (i % 64), 8)));
        invalidate();
        let cached = time(&mut |i| assert!(readable_by_query(at + 8 * (i % 64), 8)));
        let cached_read = time(&mut |i| {
            let address = at + 8 * (i % 64);
            assert!(readable_by_query(address, 8));
            // SAFETY: checked just above; our own vector.
            let v = unsafe { std::ptr::read_unaligned(address as *const u64) };
            std::hint::black_box(v);
        });
        assert!(guarded::active(), "guarded reads on for the benchmark");
        let probe = time(&mut |i| assert!(guarded::probe(at + 8 * (i % 64), 8)));
        let read = time(&mut |i| {
            std::hint::black_box(guarded::read::<u64>(at + 8 * (i % 64)).unwrap());
        });
        let faults = 20_000;
        let begin = std::time::Instant::now();
        for _ in 0..faults {
            assert!(!guarded::probe(0x7FFF_0000_0000, 8));
        }
        let fault = begin.elapsed().as_nanos() as f64 / faults as f64;
        println!(
            "a check: {fresh:.0} ns asking VirtualQuery, {cached:.1} ns from the cache, {probe:.1} ns guarded"
        );
        println!(
            "a check and an 8-byte read: {cached_read:.1} ns through the cache, {read:.1} ns guarded; a refused guarded read (a fault): {fault:.0} ns"
        );
    }
}
