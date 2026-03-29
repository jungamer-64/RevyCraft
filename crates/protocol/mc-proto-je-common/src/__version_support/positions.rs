use mc_model::BlockPos;
use num_traits::ToPrimitive;

#[must_use]
pub fn pack_block_position(position: BlockPos) -> i64 {
    let x = i64::from(position.x) & 0x3ff_ffff;
    let y = i64::from(position.y) & 0xfff;
    let z = i64::from(position.z) & 0x3ff_ffff;
    (x << 38) | (y << 26) | z
}

#[must_use]
pub fn unpack_block_position(packed: i64) -> BlockPos {
    let x = sign_extend((packed >> 38) & 0x3ff_ffff, 26);
    let y = sign_extend((packed >> 26) & 0xfff, 12);
    let z = sign_extend(packed & 0x3ff_ffff, 26);
    BlockPos::new(
        i32::try_from(x).expect("packed x should fit into i32"),
        i32::try_from(y).expect("packed y should fit into i32"),
        i32::try_from(z).expect("packed z should fit into i32"),
    )
}

#[must_use]
pub fn to_fixed_point(value: f64) -> i32 {
    rounded_f64_to_i32(value * 32.0)
}

#[must_use]
pub fn to_angle_byte(value: f32) -> i8 {
    let wrapped = value.rem_euclid(360.0);
    let scaled = rounded_f32_to_i32(wrapped * 256.0 / 360.0);
    let narrowed =
        u8::try_from(scaled.rem_euclid(256)).expect("wrapped angle should fit into byte");
    i8::from_be_bytes([narrowed])
}

const fn sign_extend(value: i64, bits: u8) -> i64 {
    let shift = 64_u8.saturating_sub(bits);
    (value << shift) >> shift
}

fn rounded_f64_to_i32(value: f64) -> i32 {
    value
        .round()
        .to_i32()
        .expect("fixed-point value should fit into i32")
}

fn rounded_f32_to_i32(value: f32) -> i32 {
    value
        .round()
        .to_i32()
        .expect("angle byte intermediate should fit into i32")
}
