pub const DEG_PIXELS: usize = 32;
pub const REV_ZOOM_LEVELS: &[&[u8]] = &[
    include_bytes!("table5.bin"),
    include_bytes!("table4.bin"),
    include_bytes!("table3.bin"),
    include_bytes!("table2.bin"),
    include_bytes!("table1.bin"),
    include_bytes!("table0.bin"),
];
pub const NUM_LEAVES: usize = 14356;
pub const UNIQUE_LEAVES_PACKED: &[u8] = include_bytes!("leaves.bin");
