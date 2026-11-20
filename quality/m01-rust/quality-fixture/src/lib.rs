#[must_use]
pub fn fixture_value() -> String {
    itoa::Buffer::new().format(42).to_owned()
}

#[cfg(test)]
mod tests;
