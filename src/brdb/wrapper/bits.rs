use crate::brdb::schema::as_brdb::{AsBrdbIter, AsBrdbValue, BrdbArrayIter};

#[derive(Clone, Default)]
pub struct BitFlags {
    vec: Vec<u8>,
    bits: usize,
}

impl BitFlags {
    pub fn new(bits: usize) -> Self {
        Self {
            vec: vec![0; (bits + 7) / 8],
            bits,
        }
    }

    pub fn get(&self, bit: usize) -> bool {
        let byte = self.vec.get(bit / 8).map(|v| *v).unwrap_or_default();
        let mask = 1 << (bit & 7);
        byte & mask > 0
    }

    pub fn set(&mut self, bit: usize, val: bool) {
        let Some(byte) = self.vec.get_mut(bit / 8) else {
            return;
        };
        let mask = 1 << (bit & 7);
        if val {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
    }

    // Push a single bit value to the end of the vector.
    pub fn push(&mut self, val: bool) {
        if self.bits >= self.vec.len() * 8 {
            self.vec.push(0);
        }
        self.set(self.bits, val);
        self.bits += 1;
    }
}

impl AsBrdbValue for BitFlags {
    fn as_brdb_struct_prop_array(
        &self,
        _schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        _prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<BrdbArrayIter, crate::brdb::errors::BrdbSchemaError> {
        Ok(self.vec.as_brdb_iter())
    }
}
