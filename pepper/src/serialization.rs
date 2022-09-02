pub trait Serializer {
    fn write(&mut self, bytes: &[u8]);
}

pub enum DeserializeError {
    InsufficientData,
    InvalidData,
}

pub trait Deserializer<'de> {
    fn read(&mut self, len: usize) -> Result<&'de [u8], DeserializeError>;
}

pub trait Serialize<'de>: Sized {
    fn serialize(&self, serializer: &mut dyn Serializer);
    fn deserialize(deserializer: &mut dyn Deserializer<'de>) -> Result<Self, DeserializeError>;
}

macro_rules! impl_serialize_num {
    ($type:ty) => {
        impl<'de> Serialize<'de> for $type {
            fn serialize(&self, serializer: &mut dyn Serializer) {
                serializer.write(&self.to_le_bytes());
            }

            fn deserialize(deserializer: &mut dyn Deserializer<'de>) -> Result<Self, DeserializeError> {
                let mut buf = [0; std::mem::size_of::<$type>()];
                let bytes = deserializer.read(buf.len())?;
                buf.clone_from_slice(bytes);
                Ok(<$type>::from_le_bytes(buf))
            }
        }
    };
}

impl_serialize_num!(u8);
impl_serialize_num!(u16);
impl_serialize_num!(u32);

impl<'de> Serialize<'de> for char {
    fn serialize(&self, serializer: &mut dyn Serializer) {
        let value = *self as u32;
        value.serialize(serializer);
    }

    fn deserialize(deserializer: &mut dyn Deserializer<'de>) -> Result<Self, DeserializeError> {
        let value = u32::deserialize(deserializer)?;
        char::try_from(value).map_err(|_| DeserializeError::InvalidData)
    }
}

impl<'de> Serialize<'de> for &'de [u8] {
    fn serialize(&self, serializer: &mut dyn Serializer) {
        let len = self.len() as u32;
        len.serialize(serializer);
        serializer.write(self);
    }

    fn deserialize(deserializer: &mut dyn Deserializer<'de>) -> Result<Self, DeserializeError> {
        let len = u32::deserialize(deserializer)?;
        deserializer.read(len as _)
    }
}

impl<'de> Serialize<'de> for &'de str {
    fn serialize(&self, serializer: &mut dyn Serializer) {
        self.as_bytes().serialize(serializer)
    }

    fn deserialize(deserializer: &mut dyn Deserializer<'de>) -> Result<Self, DeserializeError> {
        let bytes = <&[u8]>::deserialize(deserializer)?;
        std::str::from_utf8(bytes).map_err(|_| DeserializeError::InvalidData)
    }
}

impl Serializer for Vec<u8> {
    fn write(&mut self, buf: &[u8]) {
        self.extend_from_slice(buf);
    }
}

impl<'de> Deserializer<'de> for &'de [u8] {
    fn read(&mut self, len: usize) -> Result<&'de [u8], DeserializeError> {
        if len <= self.len() {
            let (read, rest) = self.split_at(len);
            *self = rest;
            Ok(read)
        } else {
            Err(DeserializeError::InsufficientData)
        }
    }
}
