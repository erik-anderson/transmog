//! Common storage and validation for generated model projections.
//! Rust owns the original value. Projection arrays borrow its strings and bytes.
use std::{any::Any, collections::BTreeMap, net::SocketAddr, path::PathBuf, sync::Arc};
use transmog_app::{AppError, ErrorCategory};

pub fn invalid(message: &str) -> AppError {
    super::error(ErrorCategory::InvalidInput, message)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Text {
    pub data: *const u8,
    pub size: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TextEntry {
    pub key: Text,
    pub value: Text,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Slice<T> {
    pub data: *const T,
    pub size: usize,
}
impl<T> Default for Slice<T> {
    fn default() -> Self {
        Self {
            data: std::ptr::null(),
            size: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Optional<T> {
    pub has_value: u32,
    pub value: T,
}

#[derive(Default)]
pub struct Arena(Vec<Box<dyn Any>>);
impl Arena {
    fn keep<T: 'static>(&mut self, value: Vec<T>) -> Slice<T> {
        let result = Slice {
            data: value.as_ptr(),
            size: value.len(),
        };
        self.0.push(Box::new(value));
        result
    }
    fn text(&mut self, text: String) -> Text {
        let bytes = self.keep(text.into_bytes());
        Text {
            data: bytes.data,
            size: bytes.size,
        }
    }
}

pub trait Project {
    type View: Copy + Default + 'static;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError>;
}
pub trait Model: Project + 'static {
    const KIND: u32;
}
pub trait FromAbi: Project + Sized {
    /// # Safety
    /// All nonempty slices must be aligned, readable and live for this call.
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError>;
}

// Owners are boxed before projection so borrowed fields never move.
pub struct Projected<T: Project> {
    pub view: T::View,
    _value: Box<T>,
    _arena: Arena,
}
impl<T: Project> Projected<T> {
    pub fn new(value: T) -> Result<Self, AppError> {
        let value = Box::new(value);
        let mut arena = Arena::default();
        let view = value.project(&mut arena)?;
        Ok(Self {
            view,
            _value: value,
            _arena: arena,
        })
    }
}

pub struct InputBudget(usize);
impl Default for InputBudget {
    fn default() -> Self {
        Self(super::MAX_INPUT_BYTES)
    }
}
impl InputBudget {
    pub fn consume(&mut self, count: usize) -> Result<(), AppError> {
        self.0 = self
            .0
            .checked_sub(count)
            .ok_or_else(|| super::error(ErrorCategory::Limit, "SDK input exceeds 262144 bytes"))?;
        Ok(())
    }
    unsafe fn slice<'a, T>(
        &mut self,
        pointer: *const T,
        length: usize,
    ) -> Result<&'a [T], AppError> {
        let bytes = length
            .checked_mul(std::mem::size_of::<T>())
            .ok_or_else(|| super::error(ErrorCategory::Limit, "SDK input length overflow"))?;
        self.consume(bytes)?;
        if length == 0 {
            return Ok(&[]);
        }
        if pointer.is_null() || !pointer.is_aligned() {
            return Err(invalid("SDK input pointer is null or misaligned"));
        }
        Ok(unsafe { std::slice::from_raw_parts(pointer, length) })
    }
}

macro_rules! scalar {
    ($($t:ty),*) => {$(
        impl Project for $t {
            type View = Self;
            fn project(&self, _: &mut Arena) -> Result<Self, AppError> { Ok(*self) }
        }
        impl FromAbi for $t {
            unsafe fn read(value: &Self, _: &mut InputBudget) -> Result<Self, AppError> { Ok(*value) }
        }
    )*};
}
scalar!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64);
impl Project for bool {
    type View = u32;
    fn project(&self, _: &mut Arena) -> Result<u32, AppError> {
        Ok(u32::from(*self))
    }
}
impl FromAbi for bool {
    unsafe fn read(value: &u32, _: &mut InputBudget) -> Result<Self, AppError> {
        match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid("SDK boolean must be zero or one")),
        }
    }
}
impl Project for str {
    type View = Text;
    fn project(&self, _: &mut Arena) -> Result<Text, AppError> {
        Ok(Text {
            data: self.as_ptr(),
            size: self.len(),
        })
    }
}
impl Project for String {
    type View = Text;
    fn project(&self, arena: &mut Arena) -> Result<Text, AppError> {
        self.as_str().project(arena)
    }
}
impl FromAbi for String {
    unsafe fn read(value: &Text, budget: &mut InputBudget) -> Result<Self, AppError> {
        let bytes = unsafe { budget.slice(value.data, value.size) }?;
        Ok(std::str::from_utf8(bytes)
            .map_err(|_| invalid("SDK text must be UTF-8"))?
            .into())
    }
}
impl Project for PathBuf {
    type View = Text;
    fn project(&self, arena: &mut Arena) -> Result<Text, AppError> {
        self.to_str()
            .ok_or_else(|| invalid("SDK paths must be UTF-8"))?
            .project(arena)
    }
}
impl FromAbi for PathBuf {
    unsafe fn read(value: &Text, budget: &mut InputBudget) -> Result<Self, AppError> {
        Ok(unsafe { String::read(value, budget) }?.into())
    }
}
impl Project for SocketAddr {
    type View = Text;
    fn project(&self, arena: &mut Arena) -> Result<Text, AppError> {
        Ok(arena.text(self.to_string()))
    }
}
impl FromAbi for SocketAddr {
    unsafe fn read(value: &Text, budget: &mut InputBudget) -> Result<Self, AppError> {
        unsafe { String::read(value, budget) }?
            .parse()
            .map_err(|_| invalid("Invalid listener socket address"))
    }
}
impl<T: Project + ?Sized> Project for Arc<T> {
    type View = T::View;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        (**self).project(arena)
    }
}
impl<T: Project> Project for [T] {
    type View = Slice<T::View>;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let values = self
            .iter()
            .map(|v| v.project(arena))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(arena.keep(values))
    }
}
impl<T: Project> Project for Vec<T> {
    type View = Slice<T::View>;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        self.as_slice().project(arena)
    }
}
impl<T: FromAbi> FromAbi for Vec<T> {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let values = unsafe { budget.slice(value.data, value.size) }?;
        values
            .iter()
            .map(|v| unsafe { T::read(v, budget) })
            .collect()
    }
}
impl<T: Project> Project for Option<T> {
    type View = Optional<T::View>;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        match self {
            Some(v) => Ok(Optional {
                has_value: 1,
                value: v.project(arena)?,
            }),
            None => Ok(Optional::default()),
        }
    }
}
impl<T: FromAbi> FromAbi for Option<T> {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        match value.has_value {
            0 => Ok(None),
            1 => Ok(Some(unsafe { T::read(&value.value, budget) }?)),
            _ => Err(invalid("SDK optional flag must be zero or one")),
        }
    }
}
impl Project for BTreeMap<String, String> {
    type View = Slice<TextEntry>;
    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {
        let entries = self
            .iter()
            .map(|(key, value)| {
                Ok(TextEntry {
                    key: key.project(arena)?,
                    value: value.project(arena)?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        Ok(arena.keep(entries))
    }
}
impl FromAbi for BTreeMap<String, String> {
    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {
        let mut result = Self::new();
        for pair in unsafe { budget.slice(value.data, value.size) }? {
            let key = unsafe { String::read(&pair.key, budget) }?;
            let value = unsafe { String::read(&pair.value, budget) }?;
            if result.insert(key, value).is_some() {
                return Err(invalid("Duplicate SDK map key"));
            }
        }
        Ok(result)
    }
}
impl Project for () {
    type View = u8;
    fn project(&self, _: &mut Arena) -> Result<u8, AppError> {
        Ok(0)
    }
}
impl Model for () {
    const KIND: u32 = 0;
}
