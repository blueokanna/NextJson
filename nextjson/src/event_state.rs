//! Shared validated state machine for the serialization event protocol.
//!
//! Serialization is an event protocol with a small state space: containers
//! open and close in order, object entries alternate key then value, arrays
//! alternate separator then value (in separator-based encodings), and exactly
//! one root value must be written.
//!
//! This state machine exists once and is used by every consumer that needs
//! to validate or observe that protocol:
//!
//! - `ser::CheckedEncoder` (the format-neutral encoder used by the binary /
//!   text format entry points);
//! - `cross_format::JsonSink` and `cross_format::CborSink` (the streaming
//!   cross-format destinations).
//!
//! The only parameter is whether the wire protocol has explicit array
//! separators. JSON-family encoders do (`,` between elements); CBOR-style
//! value-concatenated encodings do not, so their arrays never wait for a
//! separator before a value.

use crate::error::{Error, Result};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Root,
    Array,
    Object,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Array,
    Object,
}

enum Frame {
    Array { ready: bool },
    Object { pending_value: bool },
}

pub(crate) struct EventState {
    frames: Vec<Frame>,
    root_written: bool,
    separators: bool,
}

impl EventState {
    /// Create an empty state.
    ///
    /// `separators` selects the array protocol: `true` when the destination
    /// wire format has explicit separators between array elements (`,
    ///` in JSON), `false` for value-concatenated encodings (CBOR).
    pub(crate) fn new(separators: bool) -> Self {
        EventState {
            frames: Vec::new(),
            root_written: false,
            separators,
        }
    }

    /// Whether the innermost open container is an array.
    ///
    /// Sinks with explicit separators use this to decide whether the next
    /// event needs a separator before it.
    pub(crate) fn in_array(&self) -> bool {
        matches!(self.frames.last(), Some(Frame::Array { .. }))
    }

    /// Validate that a value may be written now and mark the position
    /// consumed. Returns the position the value belongs to.
    pub(crate) fn value(&mut self) -> Result<Position> {
        match self.frames.last_mut() {
            Some(Frame::Array { ready }) => {
                if *ready {
                    *ready = false;
                    Ok(Position::Array)
                } else if self.separators {
                    Err(Error::custom("array separator required before value"))
                } else {
                    Ok(Position::Array)
                }
            }
            Some(Frame::Object { pending_value }) if *pending_value => {
                *pending_value = false;
                Ok(Position::Object)
            }
            Some(Frame::Object { .. }) => Err(Error::custom("object key required before value")),
            None if self.root_written => Err(Error::custom("multiple root values")),
            None => {
                self.root_written = true;
                Ok(Position::Root)
            }
        }
    }

    /// Validate that a separator may be written now (array context, after a
    /// value) and mark the array as waiting for a value.
    pub(crate) fn separator(&mut self) -> Result<()> {
        match self.frames.last_mut() {
            Some(Frame::Array { ready }) if !*ready => {
                *ready = true;
                Ok(())
            }
            Some(Frame::Array { .. }) => Err(Error::custom("array value required after separator")),
            _ => Err(Error::custom("array separator outside array")),
        }
    }

    /// Validate that a container may open now, then push its frame.
    /// Returns the position the container belongs to.
    pub(crate) fn begin(&mut self, kind: Kind) -> Result<Position> {
        let position = self.value()?;
        self.frames.push(match kind {
            Kind::Array => Frame::Array { ready: false },
            Kind::Object => Frame::Object {
                pending_value: false,
            },
        });
        Ok(position)
    }

    /// Validate that an object key may be written now.
    pub(crate) fn key(&mut self) -> Result<()> {
        match self.frames.last_mut() {
            Some(Frame::Object { pending_value }) if !*pending_value => {
                *pending_value = true;
                Ok(())
            }
            Some(Frame::Object { .. }) => Err(Error::custom("object value required after key")),
            _ => Err(Error::custom("object key outside object")),
        }
    }

    /// Validate that the innermost container may close now, then pop it.
    pub(crate) fn end(&mut self, kind: Kind) -> Result<()> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| Error::custom("container end without matching start"))?;
        match (kind, frame) {
            (Kind::Array, Frame::Array { ready: true }) => {
                Err(Error::custom("array ended after separator without value"))
            }
            (Kind::Array, Frame::Array { ready: false }) => Ok(()),
            (
                Kind::Object,
                Frame::Object {
                    pending_value: false,
                },
            ) => Ok(()),
            (
                Kind::Object,
                Frame::Object {
                    pending_value: true,
                },
            ) => Err(Error::custom("object ended before keyed value")),
            (Kind::Object, Frame::Array { .. }) => {
                Err(Error::custom("mismatched object end inside array"))
            }
            (Kind::Array, Frame::Object { .. }) => {
                Err(Error::custom("mismatched array end inside object"))
            }
        }
    }

    /// Validate that the stream ended with exactly one complete root value.
    pub(crate) fn finish(&self) -> Result<()> {
        if !self.root_written {
            return Err(Error::custom("encoder did not receive a root value"));
        }
        if !self.frames.is_empty() {
            return Err(Error::custom("encoder finished inside a container"));
        }
        Ok(())
    }
}
