use std::mem;
use std::marker;

#[derive(Debug)]
pub struct EndOfBufferError;

#[derive(Clone,Copy,Debug)]
pub struct Buffer<'a> {
    pub inner: &'a[u8]
}


/// Trait implemented for types that can read itself from a Buffer
///
/// Should be implemented with zero-copying
pub trait Parse<'a>
    where Self: marker::Sized
{
    fn parse(buf: &mut Buffer<'a>) -> Result<Self, EndOfBufferError>;
}




impl<'a> Buffer<'a> {

    pub fn new(slice: &'a[u8]) -> Self {
        Buffer {
            inner: slice
        }
    }



    pub fn len(self) -> usize {
        self.inner.len()
    }


    pub fn consumed_since(self, original: Buffer) -> Buffer {
        let len = original.inner.len() - self.inner.len();
        Buffer {
            inner: &original.inner[..len]
        }
    }

    pub fn parse_compact_size(&mut self) -> Result<usize, EndOfBufferError> {
        let byte1 = { try!(u8::parse(self)) };
        Ok(match byte1 {
            0xff => { try!(u64::parse(self)) as usize },
            0xfe => { try!(u32::parse(self)) as usize },
            0xfd => { try!(u16::parse(self)) as usize },
            _ => byte1 as usize
        })
    }

    pub fn parse_bytes(&mut self, count: usize) -> Result<&'a[u8], EndOfBufferError> {
        if self.inner.len() < count {
            return Err(EndOfBufferError);
        }

        let result = &self.inner[..count];
        self.inner = &self.inner[count..];

        Ok(result)
    }

    pub fn parse_compact_size_bytes(&mut self) -> Result<&'a[u8], EndOfBufferError> {
        let count = try!(self.parse_compact_size());

        self.parse_bytes(count)
    }

}



impl<'a, T : Parse<'a>> Parse<'a> for Vec<T> {
    fn parse(buffer: &mut Buffer<'a>) -> Result<Vec<T>, EndOfBufferError> {

        let count = try!(buffer.parse_compact_size());
        let mut result: Vec<T> = Vec::with_capacity(count);
        for _ in 0..count {
            result.push(try!(T::parse(buffer)));
        }
        Ok(result)
    }
}



macro_rules! impl_parse_primitive {
    ($prim_type: ty) =>

    (
        impl<'a> Parse<'a> for $prim_type {
            fn parse(buffer: &mut Buffer<'a>) -> Result<$prim_type, EndOfBufferError> {
                let sz = mem::size_of::<$prim_type>();
                if buffer.inner.len() < sz {
                    return Err(EndOfBufferError);
                }

                // Shift-n-fold
                let result = (0..sz)
                    .map(|n| (buffer.inner[n] as $prim_type) << (8* n) )
                    .fold(0, |a,b| a | b);

                buffer.inner = &buffer.inner[sz..];

                Ok(result)
            }
        }
    )
}

impl_parse_primitive!(u32);
impl_parse_primitive!(i64);
impl_parse_primitive!(i32);
impl_parse_primitive!(u8);
impl_parse_primitive!(u16);
impl_parse_primitive!(u64);



#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn test_primitive() {
        let x = &[0xff_u8, 0x00_u8, 0x00_u8, 0x00_u8];
        let mut buf = Buffer { inner: x };
        let org_buf = buf;

        assert_eq!(u32::parse(&mut buf).unwrap(), 0xff_u32);

        assert_eq!(buf.len(), 0);
        assert_eq!(buf.consumed_since(org_buf).len(), 4);
    }
}


