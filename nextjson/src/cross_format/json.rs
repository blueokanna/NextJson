use crate::cross_format::{ContainerKind, EventSink, StructureState, ValuePosition};
use crate::encoding::{EncodeConfig, Encoder};
use crate::error::Result;
use crate::number::Number;
use crate::write::Write;

/// JSON destination for [`EventSink`] streams.
///
/// The sink validates event order independently of the source and uses the
/// native NextJson encoder for escaping and number formatting.
pub struct JsonSink<W: Write> {
    encoder: Encoder<W>,
    structure: StructureState,
}

impl<W: Write> JsonSink<W> {
    /// Create a compact JSON event sink.
    pub fn new(writer: W) -> Self {
        Self::with_config(writer, EncodeConfig::compact())
    }

    /// Create a JSON event sink with explicit output configuration.
    pub fn with_config(writer: W, config: EncodeConfig) -> Self {
        JsonSink {
            encoder: Encoder::with_config(writer, config),
            structure: StructureState::new(),
        }
    }

    /// Validate completion, flush output, and return the writer.
    pub fn finish(self) -> Result<W> {
        self.structure.finish()?;
        self.encoder.finish()
    }

    fn prepare_value(&mut self) -> Result<()> {
        if matches!(self.structure.value()?, ValuePosition::Array) {
            self.encoder.separator()?;
        }
        Ok(())
    }

    fn prepare_container(&mut self, kind: ContainerKind) -> Result<()> {
        if matches!(self.structure.begin(kind)?, ValuePosition::Array) {
            self.encoder.separator()?;
        }
        Ok(())
    }
}

impl<W: Write> EventSink for JsonSink<W> {
    fn null(&mut self) -> Result<()> {
        self.prepare_value()?;
        self.encoder.write_null()
    }

    fn boolean(&mut self, value: bool) -> Result<()> {
        self.prepare_value()?;
        self.encoder.write_bool(value)
    }

    fn number(&mut self, value: Number) -> Result<()> {
        self.prepare_value()?;
        self.encoder.write_number(&value)
    }

    fn string(&mut self, value: &str) -> Result<()> {
        self.prepare_value()?;
        self.encoder.write_str(value)
    }

    fn begin_array(&mut self) -> Result<()> {
        self.prepare_container(ContainerKind::Array)?;
        self.encoder.begin_array()
    }

    fn end_array(&mut self) -> Result<()> {
        self.structure.end(ContainerKind::Array)?;
        self.encoder.end_array()
    }

    fn begin_object(&mut self) -> Result<()> {
        self.prepare_container(ContainerKind::Object)?;
        self.encoder.begin_object()
    }

    fn object_key(&mut self, key: &str) -> Result<()> {
        self.structure.key()?;
        self.encoder.key(key)
    }

    fn end_object(&mut self) -> Result<()> {
        self.structure.end(ContainerKind::Object)?;
        self.encoder.end_object()
    }
}
