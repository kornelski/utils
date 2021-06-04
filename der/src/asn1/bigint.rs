//! "Big" ASN.1 `INTEGER` types.
// TODO(tarcieri): completely replace `UIntBytes` with `crypto_bigint::UInt`
// It should be possible to leverage the encoding logic in `asn1::integer::uint`

use crate::{
    asn1::Any, ByteSlice, Encodable, Encoder, Error, ErrorKind, Header, Length, Result, Tag, Tagged,
};
use core::{
    convert::{TryFrom, TryInto},
    marker::PhantomData,
};
use crypto_bigint::{generic_array::GenericArray, ArrayEncoding, UInt};
use typenum::{NonZero, Unsigned};

/// Alias for getting the size of a [`UInt`] with the given number of limbs in bytes.
type ByteSize<const LIMBS: usize> = <UInt<LIMBS> as ArrayEncoding>::ByteSize;

/// "Big" unsigned ASN.1 `INTEGER` type.
///
/// Provides direct access to the underlying big endian bytes which comprise an
/// unsigned integer value.
///
/// Intended for use cases like very large integers that are used in
/// cryptographic applications (e.g. keys, signatures).
///
/// Generic over a `Size` value (e.g. [`der::consts::U64`][`typenum::U64`]),
/// indicating the size of an integer in bytes.
///
/// Currently supported sizes are 1 - 512 bytes.
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[cfg_attr(docsrs, doc(cfg(feature = "bigint")))]
pub struct UIntBytes<'a, N: Unsigned + NonZero> {
    /// Inner value
    inner: ByteSlice<'a>,

    /// Integer size in bytes
    size: PhantomData<N>,
}

impl<'a, N> UIntBytes<'a, N>
where
    N: Unsigned + NonZero,
{
    /// Create a new [`UIntBytes`] from a byte slice.
    ///
    /// Slice may be less than or equal to `N` bytes.
    pub fn new(mut bytes: &'a [u8]) -> Result<Self> {
        // Remove leading zeroes
        while bytes.get(0).cloned() == Some(0) {
            bytes = &bytes[1..];
        }

        if bytes.len() > N::to_usize() {
            return Err(ErrorKind::Length { tag: Self::TAG }.into());
        }

        ByteSlice::new(bytes)
            .map(|inner| Self {
                inner,
                size: PhantomData,
            })
            .map_err(|_| ErrorKind::Length { tag: Self::TAG }.into())
    }

    /// Borrow the inner byte slice which contains the least significant bytes
    /// of a big endian integer value with all leading zeros stripped, and may
    /// be any length from empty (i.e. zero) to `N` bytes.
    pub fn as_bytes(&self) -> &'a [u8] {
        self.inner.as_bytes()
    }

    /// Get the length of this [`UIntBytes`] in bytes.
    pub fn len(&self) -> Length {
        self.inner.len()
    }

    /// Is the inner byte slice empty?
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Get the length of the inner integer value when encoded.
    fn inner_len(self) -> Result<Length> {
        self.len()
            + match self.inner.as_ref().get(0).cloned() {
                Some(n) if n >= 0x80 => 1u8, // Needs leading `0`
                None => 1u8,                 // Needs leading `0`
                _ => 0u8,                    // No leading `0`
            }
    }
}

impl<'a, N> From<&UIntBytes<'a, N>> for UIntBytes<'a, N>
where
    N: Unsigned + NonZero,
{
    fn from(value: &UIntBytes<'a, N>) -> UIntBytes<'a, N> {
        *value
    }
}

impl<'a, N> TryFrom<Any<'a>> for UIntBytes<'a, N>
where
    N: Unsigned + NonZero,
{
    type Error = Error;

    fn try_from(any: Any<'a>) -> Result<UIntBytes<'a, N>> {
        any.tag().assert_eq(Tag::Integer)?;
        let mut bytes = any.as_bytes();

        // Disallow a leading byte which would overflow a signed
        // ASN.1 integer (since this is a "uint" type).
        // We expect all such cases to have a leading `0x00` byte
        // (see comment below)
        if let Some(byte) = bytes.get(0).cloned() {
            if byte > 0x80 {
                return Err(ErrorKind::Value { tag: Self::TAG }.into());
            }
        }

        // The `INTEGER` type always encodes a signed value, so for unsigned
        // values the leading `0x00` byte may need to be removed.
        // TODO(tarcieri): validate leading 0 byte was required
        if bytes.len() > N::to_usize() {
            if bytes.len() != N::to_usize().checked_add(1).expect("overflow") {
                return Err(ErrorKind::Length { tag: Self::TAG }.into());
            }

            if bytes.get(0).cloned() != Some(0) {
                return Err(ErrorKind::Value { tag: Self::TAG }.into());
            }

            bytes = &bytes[1..];
        }

        Self::new(bytes)
    }
}

impl<'a, N> Encodable for UIntBytes<'a, N>
where
    N: Unsigned + NonZero,
{
    fn encoded_len(&self) -> Result<Length> {
        self.inner_len()?.for_tlv()
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<()> {
        Header::new(Self::TAG, self.inner_len()?)?.encode(encoder)?;

        // Add leading `0x00` byte if required
        if self.inner_len()? > self.len() {
            encoder.byte(0)?;
        }

        encoder.bytes(self.as_bytes())
    }
}

impl<'a, N> Tagged for UIntBytes<'a, N>
where
    N: Unsigned + NonZero,
{
    const TAG: Tag = Tag::Integer;
}

impl<'a, const LIMBS: usize> TryFrom<Any<'a>> for UInt<LIMBS>
where
    UInt<LIMBS>: ArrayEncoding,
    ByteSize<LIMBS>: Unsigned + NonZero,
{
    type Error = Error;

    fn try_from(any: Any<'a>) -> Result<UInt<LIMBS>> {
        // TODO(tarcieri): get rid of intermediate `UIntBytes` conversion
        let uint_bytes = UIntBytes::<ByteSize<LIMBS>>::try_from(any)?;
        let mut array = GenericArray::default();
        let offset = array.len().saturating_sub(uint_bytes.len().try_into()?);
        array[offset..].copy_from_slice(uint_bytes.as_bytes());
        Ok(UInt::from_be_byte_array(&array))
    }
}

impl<'a, const LIMBS: usize> Encodable for UInt<LIMBS>
where
    UInt<LIMBS>: ArrayEncoding,
    ByteSize<LIMBS>: Unsigned + NonZero,
{
    fn encoded_len(&self) -> Result<Length> {
        // TODO(tarcieri): more efficient length calculation
        let array = self.to_be_byte_array();
        UIntBytes::<ByteSize<LIMBS>>::new(&array)?.encoded_len()
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<()> {
        let array = self.to_be_byte_array();
        UIntBytes::<ByteSize<LIMBS>>::new(&array)?.encode(encoder)
    }
}

impl<'a, const LIMBS: usize> Tagged for UInt<LIMBS>
where
    UInt<LIMBS>: ArrayEncoding,
    ByteSize<LIMBS>: Unsigned + NonZero,
{
    const TAG: Tag = Tag::Integer;
}

#[cfg(test)]
mod tests {
    use super::UIntBytes;
    use crate::{
        asn1::{integer::tests::*, Any},
        Decodable, ErrorKind, Result, Tag,
    };
    use core::convert::TryInto;

    // TODO(tarcieri): tests for more integer sizes
    type BigU8<'a> = UIntBytes<'a, typenum::U1>;
    type BigU16<'a> = UIntBytes<'a, typenum::U2>;

    /// Parse a `BitU1` from an ASN.1 `Any` value to test decoding behaviors.
    fn parse_bigu8_from_any(bytes: &[u8]) -> Result<BigU8<'_>> {
        Any::new(Tag::Integer, bytes)?.try_into()
    }

    #[test]
    fn decode_empty() {
        let x = parse_bigu8_from_any(&[]).unwrap();
        assert_eq!(x.as_bytes(), &[]);
    }

    #[test]
    fn decode_bigu8() {
        assert!(BigU8::from_der(I0_BYTES).unwrap().is_empty());
        assert_eq!(&[127], BigU8::from_der(I127_BYTES).unwrap().as_bytes());
        assert_eq!(&[128], BigU8::from_der(I128_BYTES).unwrap().as_bytes());
        assert_eq!(&[255], BigU8::from_der(I255_BYTES).unwrap().as_bytes());
    }

    #[test]
    fn decode_bigu16() {
        assert!(BigU16::from_der(I0_BYTES).unwrap().is_empty());
        assert_eq!(&[127], BigU16::from_der(I127_BYTES).unwrap().as_bytes());
        assert_eq!(&[128], BigU16::from_der(I128_BYTES).unwrap().as_bytes());
        assert_eq!(&[255], BigU16::from_der(I255_BYTES).unwrap().as_bytes());

        assert_eq!(
            &[0x01, 0x00],
            BigU16::from_der(I256_BYTES).unwrap().as_bytes()
        );

        assert_eq!(
            &[0x7F, 0xFF],
            BigU16::from_der(I32767_BYTES).unwrap().as_bytes()
        );
    }

    #[test]
    fn reject_oversize_without_extra_zero() {
        let err = parse_bigu8_from_any(&[0x81]).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::Value { tag: Tag::Integer });
    }
}