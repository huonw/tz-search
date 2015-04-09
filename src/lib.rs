//! Compute timezones of points on the Earth.
//!
//! This is a direct port of
//! [bradfitz/latlong](https://github.com/bradfitz/latlong), and hence
//! the same bonuses/caveats apply:
//!
//! > It tries to have a small binary size (~360 KB), low memory footprint
//! > (~1 MB), and incredibly fast lookups (~0.5 microseconds). It does not
//! > try to be perfectly accurate when very close to borders.
//!
//! [Source](https://github.com/huonw/tz-search).
//!
//! # Installation
//!
//! Add the following to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! tz-search = "0.1"
//! ```
//!
//! # Examples
//!
//! ```rust
//! assert_eq!(tz_search::lookup(-33.79, 151.17).unwrap(),
//!            "Australia/Sydney");
//!
//! // in the ocean
//! assert_eq!(tz_search::lookup(0.0, 0.0), None);
//! ```


extern crate flate2;
extern crate rustc_serialize;
extern crate byteorder;

use std::{cmp, mem, sync};
use std::sync::atomic;
use std::io::prelude::*;
use std::io::BufReader;
use byteorder::{BigEndian, ReadBytesExt};

#[allow(warnings)]
mod tables;

/// Attempt to compute the timezone that the point `lat`, `long`
/// lies in.
///
/// The latitude `lat` should lie in the range `[-90, 90]` with
/// negative representing south, and the longitude should lie in
/// the range `[-180, 180]` with negative representing west. This
/// will fail (return `None`) if the point lies in the ocean.
///
/// # Panics
///
/// `lookup` will panic if either of the two ranges above are
/// violated.
///
/// # Examples
///
/// ```rust
/// assert_eq!(tz_search::lookup(-33.79, 151.17).unwrap(),
///            "Australia/Sydney");
///
/// // in the ocean
/// assert_eq!(tz_search::lookup(0.0, 0.0), None);
/// ```
pub fn lookup(lat: f64, lon: f64) -> Option<String> {
    static SHARED: atomic::AtomicUsize = atomic::ATOMIC_USIZE_INIT;
    static ONCE: sync::Once = sync::ONCE_INIT;

    ONCE.call_once(|| {
        let s = Box::new(TzSearch::new());
        SHARED.store(unsafe {mem::transmute(s)}, atomic::Ordering::Relaxed);
    });

    let ptr = SHARED.load(atomic::Ordering::Relaxed);
    assert!(ptr != 0);
    let s = unsafe {&*(ptr as *const TzSearch)};
    s.lookup(lat, lon)
}

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TileKey(u32);
impl TileKey {
    fn new(size: u8, x: u16, y: u16) -> TileKey {
        let size = size as u32;
        let x = x as u32;
        let y = y as u32;
        TileKey((size % 8) << 28 | (y % (1<<14)) << 14 | (x % (1<<14)))
    }
}

#[derive(Debug)]
struct TileLooker {
    tile: TileKey,
    idx: u16,
}
#[derive(Debug)]
struct ZoomLevel {
    tiles: Vec<TileLooker>
}
#[derive(Debug)]

/// All the information required for efficient time-zone lookups.
///
/// Unless you need absolutely strict control over memory use, you
/// probably want to call the top-level `lookup`, rather than going
/// via this.
pub struct TzSearch {
    leaves: Vec<Zone>,
    zoom_levels: Vec<ZoomLevel>,
}

enum Zone {
    StaticZone(String),
    OneBitTile([u16; 2], [u8; 8]),
    Pixmap([u8; 128])
}
impl std::fmt::Debug for Zone {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self {
            Zone::StaticZone(ref s) => write!(f, "StaticZone({:?})", s),
            Zone::OneBitTile(a, b) => write!(f, "OneBitTile({:?}, {:?})", &a[..], &b[..]),
            Zone::Pixmap(c) => write!(f, "Pixmap({:?})", &c[..]),
        }
    }
}


impl TzSearch {
    /// Create a new `TzSearch`.
    ///
    /// This is very expensive: the initialisation routine takes a
    /// long time, and the resulting structure uses a lot of
    /// memory. Hence, this should be called as rarely as
    /// possible.
    ///
    /// The free-standing `lookup` function internally manages
    /// creating exactly one of these, and should be preferred.
    pub fn new() -> TzSearch {
        let mut zoom_levels = vec![];
        for data in tables::REV_ZOOM_LEVELS.iter().rev() {
            let unb64d = rustc_serialize::base64::FromBase64::from_base64(*data).unwrap();
            let mut slurp = vec![];
            flate2::read::GzDecoder::new(&*unb64d).unwrap().read_to_end(&mut slurp).unwrap();
            assert_eq!(slurp.len() % 6, 0);
            let count = slurp.len() / 6;

            let mut slurp = &slurp[..];
            let mut tiles = Vec::with_capacity(count);
            for _ in 0..count {
                tiles.push(TileLooker {
                    tile: TileKey(slurp.read_u32::<BigEndian>().unwrap()),
                    idx: slurp.read_u16::<BigEndian>().unwrap(),
                })
            }

            zoom_levels.push(ZoomLevel { tiles: tiles })
        }
        let mut leaves = Vec::with_capacity(tables::NUM_LEAVES);
        let unb64d = rustc_serialize::base64::FromBase64::from_base64(tables::UNIQUE_LEAVES_PACKED)
            .unwrap();
        let mut ungz = BufReader::new(flate2::read::GzDecoder::new(&*unb64d).unwrap());
        let mut buf = [0; 128];
        for _ in 0..tables::NUM_LEAVES {
            let zone = match ungz.read_u8().unwrap() {
                b'S' => {
                    let mut zone_name = vec![];
                    ungz.read_until(0, &mut zone_name).unwrap();
                    if zone_name.last() == Some(&0) { zone_name.pop(); }
                    Zone::StaticZone(String::from_utf8(zone_name).unwrap())
                }
                b'2' => {
                    let idx = [ungz.read_u16::<BigEndian>().unwrap(),
                               ungz.read_u16::<BigEndian>().unwrap()];
                    let bits = ungz.read_u64::<BigEndian>().unwrap();
                    let mut rows = [0; 8];
                    for (y, place) in rows.iter_mut().enumerate() {
                        for x in 0..8 {
                            if bits & (1 << (y * 8 + x)) != 0 {
                                *place |= 1 << x
                            }
                        }
                    }
                    Zone::OneBitTile(idx, rows)
                }
                b'P' => {
                    let mut i = 0;
                    while i < 128 {
                        i += ungz.read(&mut buf[i..]).unwrap()
                    }
                    Zone::Pixmap(buf)
                }
                _ => panic!("unknown leaf type")
            };
            leaves.push(zone)
        }

        TzSearch {
            zoom_levels: zoom_levels,
            leaves: leaves
        }
    }

    /// Attempt to compute the timezone that the point `lat`, `long`
    /// lies in.
    ///
    /// The latitude `lat` should lie in the range `[-90, 90]` with
    /// negative representing south, and the longitude should lie in
    /// the range `[-180, 180]` with negative representing west. This
    /// will fail (return `None`) if the point lies in the ocean.
    ///
    /// See also: the `lookup` function at the top-level.
    ///
    /// # Panics
    ///
    /// `lookup` will panic if either of the two ranges above are
    /// violated.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let s = tz_search::TzSearch::new();
    /// let (lat, long) = (-33.79, 151.17);
    /// assert_eq!(s.lookup(-33.79, 151.17).unwrap(),
    ///            "Australia/Sydney");
    /// ```
    pub fn lookup(&self, lat: f64, long: f64) -> Option<String> {
        fn clamp(x: isize, lim: isize) -> usize {
            cmp::max(0, cmp::min(lim * tables::DEG_PIXELS as isize, x)) as usize
        }
        assert!(-90.0 <= lat && lat <= 90.0);
        assert!(-180.0 <= long && long <= 180.0);
        let x = ((long + 180.0) * (tables::DEG_PIXELS as f64)) as isize;
        let y = ((90.0 - lat) * (tables::DEG_PIXELS as f64)) as isize;
        let x = clamp(x, 360);
        let y = clamp(y, 180);

        self.lookup_pixel(x, y)
    }
    fn lookup_pixel(&self, x: usize, y: usize) -> Option<String> {
        for level in (0..6).rev() {
            let shift = 3 + level;
            let xt = x >> shift;
            let yt = y >> shift;
            let tk = TileKey::new(level, xt as u16, yt as u16);
            if let Some(ret) = self.zoom_level_lookup(&self.zoom_levels[level as usize], x, y, tk) {
                return ret
            }
        }
        None
    }

    fn zone_lookup(&self, zone: &Zone, x: usize, y: usize, tk: TileKey) -> Option<Option<String>> {
        match *zone {
            Zone::StaticZone(ref s) => Some(Some(s.clone())),
            Zone::OneBitTile(idxs, rows) => {
                let idx = if rows[y & 7] & (1 << (x & 7)) != 0 {
                    idxs[1]
                } else {
                    idxs[0]
                };
                self.zone_lookup(&self.leaves[idx as usize], x, y, tk)
            }
            Zone::Pixmap(ref p) => {
                let xx = x & 7;
                let yy = y & 7;
                let i = 2 * (yy * 8 + xx);
                let idx = ((p[i] as usize) << 8) + p[i+1] as usize;
                const OCEAN_INDEX: usize = 0xFFFF;
                if idx == OCEAN_INDEX {
                    Some(None)
                } else {
                    self.zone_lookup(&self.leaves[idx], x, y, tk)
                }
            }
        }
    }

    fn zoom_level_lookup(&self, zl: &ZoomLevel, x: usize, y: usize, tk: TileKey)
                         -> Option<Option<String>>
    {
        let pos = zl.tiles.binary_search_by(|t| t.tile.cmp(&tk)).unwrap_or_else(|x| x);

        match zl.tiles.get(pos) {
            Some(tl) if tl.tile == tk => self.zone_lookup(&self.leaves[tl.idx as usize], x, y, tk),
            _ => None
        }
    }
}


#[cfg(test)]
mod tests {
    use super::{lookup, TzSearch};

    #[test]
    fn loads_ok() {
        let _searcher = TzSearch::new();
    }

    #[test]
    fn test_lookup_lat_long() {
        let searcher = TzSearch::new();
        let tests = [(37.7833, -122.4167, Some("America/Los_Angeles")),
                     (-33.79, 151.17, Some("Australia/Sydney"))];
        for &(lat, lon, want) in &tests {
            let want = want.map(|s| s.to_string());
            assert_eq!(searcher.lookup(lat, lon), want);
            assert_eq!(lookup(lat, lon), want)
        }
    }
    #[test]
    fn test_lookup_pixel() {
        let searcher = TzSearch::new();
        let tests = [
            (9200, 2410, Some("Asia/Phnom_Penh")),
            (9047, 2488, Some("Asia/Phnom_Penh")),

            // one-bit leaf tile:
            (9290, 530, Some("Asia/Krasnoyarsk")),
            (9290, 531, Some("Asia/Yakutsk")),

            // four-bit tile:
            (2985, 1654, Some("America/Indiana/Vincennes")),
            (2986, 1654, Some("America/Indiana/Marengo")),
            (2986, 1655, Some("America/Indiana/Tell_City")),

            // Empty tile:
            (4000, 2000, None),

            // Big 1-color tile in ocean with island:
            (3687, 1845, Some("Atlantic/Bermuda")),
            // Same, but off Oregon coast:
            (1747, 1486, Some("America/Los_Angeles")),

            // Little solid tile:
            (2924, 2316, Some("America/Belize")),
            ];

        for &(lat, lon, ref want) in &tests {
            assert_eq!(searcher.lookup_pixel(lat, lon), want.map(|s| s.to_string()));
        }
    }
}
