//! Local trace identity shared by persistence, application and transport boundaries.
use serde::{Serialize, Serializer, ser::SerializeStruct as _};

#[derive(Clone, Copy, Debug)]
pub struct TraceLink {
    pub trace: [u8; 16],
    pub span: [u8; 8],
}

impl Serialize for TraceLink {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut link = serializer.serialize_struct("TraceLink", 2)?;
        link.serialize_field(
            "traceId",
            &format_args!("{:032x}", u128::from_be_bytes(self.trace)),
        )?;
        link.serialize_field(
            "spanId",
            &format_args!("{:016x}", u64::from_be_bytes(self.span)),
        )?;
        link.end()
    }
}
