use crate::cross_format::EventSink;
use crate::encoding::{EncodeConfig, Encoder};
use crate::error::Result;
use crate::event_state::{EventState, Kind};
use crate::number::Number;
use crate::write::Write;

/// JSON destination for [`EventSink`] streams.
///
/// The sink validates event order through the shared event-protocol state
/// machine and uses the native NextJson encoder for escaping and number
/// formatting.
pub struct JsonSink<W: Write> {
    encoder: Encoder<W>,
    structure: EventState,
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
            structure: EventState::new(true),
        }
    }

    /// Validate completion, flush output, and return the writer.
    pub fn finish(self) -> Result<W> {
        self.structure.finish()?;
        self.encoder.finish()
    }

    fn prepare_value(&mut self) -> Result<()> {
        if self.structure.in_array() {
            self.structure.separator()?;
            self.encoder.separator()?;
        }
        self.structure.value()?;
        Ok(())
    }

    fn prepare_container(&mut self, kind: Kind) -> Result<()> {
        if self.structure.in_array() {
            self.structure.separator()?;
            self.encoder.separator()?;
        }
        self.structure.begin(kind)?;
        match kind {
            Kind::Array => self.encoder.begin_array(),
            Kind::Object => self.encoder.begin_object(),
        }
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
        self.prepare_container(Kind::Array)?;
        Ok(())
    }

    fn end_array(&mut self) -> Result<()> {
        self.structure.end(Kind::Array)?;
        self.encoder.end_array()
    }

    fn begin_object(&mut self) -> Result<()> {
        self.prepare_container(Kind::Object)?;
        Ok(())
    }

    fn object_key(&mut self, key: &str) -> Result<()> {
        self.structure.key()?;
        self.encoder.key(key)
    }

    fn end_object(&mut self) -> Result<()> {
        self.structure.end(Kind::Object)?;
        self.encoder.end_object()
    }
}
