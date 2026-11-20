#[must_use]
pub fn clippy_fixture(flag: bool) -> bool {
    if flag { true } else { false }
}

#[must_use]
pub fn restriction_fixture(value: Option<u8>) -> u8 {
    value.unwrap()
}
