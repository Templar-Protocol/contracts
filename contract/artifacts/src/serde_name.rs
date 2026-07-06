use std::fmt;

use serde::Serialize;

pub(crate) fn unit_variant_name<T: Serialize>(value: &T) -> Result<&'static str, NameError> {
    value.serialize(UnitVariantNameSerializer)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct NameError;

impl fmt::Display for NameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("expected a unit enum variant")
    }
}

impl std::error::Error for NameError {}

impl serde::ser::Error for NameError {
    fn custom<T: fmt::Display>(_message: T) -> Self {
        Self
    }
}

struct UnitVariantNameSerializer;

macro_rules! unsupported_serializer_method {
    ($method:ident($($arg:ident: $ty:ty),* $(,)?)) => {
        fn $method(self, $($arg: $ty),*) -> Result<Self::Ok, Self::Error> {
            $(let _ = $arg;)*
            Err(NameError)
        }
    };
}

macro_rules! unsupported_compound_serializer_method {
    ($method:ident -> $return_ty:ty; $($arg:ident: $ty:ty),* $(,)?) => {
        fn $method(self, $($arg: $ty),*) -> Result<$return_ty, Self::Error> {
            $(let _ = $arg;)*
            Err(NameError)
        }
    };
}

impl serde::Serializer for UnitVariantNameSerializer {
    type Ok = &'static str;
    type Error = NameError;
    type SerializeSeq = serde::ser::Impossible<Self::Ok, Self::Error>;
    type SerializeTuple = serde::ser::Impossible<Self::Ok, Self::Error>;
    type SerializeTupleStruct = serde::ser::Impossible<Self::Ok, Self::Error>;
    type SerializeTupleVariant = serde::ser::Impossible<Self::Ok, Self::Error>;
    type SerializeMap = serde::ser::Impossible<Self::Ok, Self::Error>;
    type SerializeStruct = serde::ser::Impossible<Self::Ok, Self::Error>;
    type SerializeStructVariant = serde::ser::Impossible<Self::Ok, Self::Error>;

    unsupported_serializer_method!(serialize_bool(value: bool));
    unsupported_serializer_method!(serialize_i8(value: i8));
    unsupported_serializer_method!(serialize_i16(value: i16));
    unsupported_serializer_method!(serialize_i32(value: i32));
    unsupported_serializer_method!(serialize_i64(value: i64));
    unsupported_serializer_method!(serialize_i128(value: i128));
    unsupported_serializer_method!(serialize_u8(value: u8));
    unsupported_serializer_method!(serialize_u16(value: u16));
    unsupported_serializer_method!(serialize_u32(value: u32));
    unsupported_serializer_method!(serialize_u64(value: u64));
    unsupported_serializer_method!(serialize_u128(value: u128));
    unsupported_serializer_method!(serialize_f32(value: f32));
    unsupported_serializer_method!(serialize_f64(value: f64));
    unsupported_serializer_method!(serialize_char(value: char));
    unsupported_serializer_method!(serialize_str(value: &str));
    unsupported_serializer_method!(serialize_bytes(value: &[u8]));
    unsupported_serializer_method!(serialize_unit_struct(name: &'static str));

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(NameError)
    }

    fn serialize_some<T: ?Sized + serde::Serialize>(
        self,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        let _ = value;
        Err(NameError)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(NameError)
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        let _ = (name, variant_index);
        Ok(variant)
    }

    fn serialize_newtype_struct<T: ?Sized + serde::Serialize>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        let _ = (name, value);
        Err(NameError)
    }

    fn serialize_newtype_variant<T: ?Sized + serde::Serialize>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        let _ = (name, variant_index, variant, value);
        Err(NameError)
    }

    unsupported_compound_serializer_method!(serialize_seq -> Self::SerializeSeq; len: Option<usize>);
    unsupported_compound_serializer_method!(serialize_tuple -> Self::SerializeTuple; len: usize);
    unsupported_compound_serializer_method!(serialize_tuple_struct -> Self::SerializeTupleStruct; name: &'static str, len: usize);
    unsupported_compound_serializer_method!(serialize_tuple_variant -> Self::SerializeTupleVariant; name: &'static str, variant_index: u32, variant: &'static str, len: usize);
    unsupported_compound_serializer_method!(serialize_map -> Self::SerializeMap; len: Option<usize>);
    unsupported_compound_serializer_method!(serialize_struct -> Self::SerializeStruct; name: &'static str, len: usize);
    unsupported_compound_serializer_method!(serialize_struct_variant -> Self::SerializeStructVariant; name: &'static str, variant_index: u32, variant: &'static str, len: usize);
}
