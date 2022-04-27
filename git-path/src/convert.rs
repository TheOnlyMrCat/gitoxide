use std::{
    borrow::Cow,
    ffi::OsStr,
    path::{Path, PathBuf},
};

#[derive(Debug)]
/// The error type returned by [`into_bytes()`] and others may suffer from failed conversions from or to bytes.
pub struct Utf8Error;

impl std::fmt::Display for Utf8Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Could not convert to UTF8 or from UTF8 due to ill-formed input")
    }
}

impl std::error::Error for Utf8Error {}

/// Like [`into_bytes()`], but takes `OsStr` as input for a lossless, but fallible, conversion.
pub fn os_str_into_bytes(path: &OsStr) -> Result<&[u8], Utf8Error> {
    let path = into_bytes(Cow::Borrowed(path.as_ref()))?;
    match path {
        Cow::Borrowed(path) => Ok(path),
        Cow::Owned(_) => unreachable!("borrowed cows stay borrowed"),
    }
}

/// Convert the given path either into its raw bytes on unix or its UTF8 encoded counterpart on windows.
///
/// On windows, if the source Path contains ill-formed, lone surrogates, the UTF-8 conversion will fail
/// causing `Utf8Error` to be returned.
pub fn into_bytes<'a>(path: impl Into<Cow<'a, Path>>) -> Result<Cow<'a, [u8]>, Utf8Error> {
    let path = path.into();
    let utf8_bytes = match path {
        Cow::Owned(path) => Cow::Owned({
            #[cfg(unix)]
            let p = {
                use std::os::unix::ffi::OsStringExt;
                path.into_os_string().into_vec()
            };
            #[cfg(not(unix))]
            let p: Vec<_> = path.into_os_string().into_string().map_err(|_| Utf8Error)?.into();
            p
        }),
        Cow::Borrowed(path) => Cow::Borrowed({
            #[cfg(unix)]
            let p = {
                use std::os::unix::ffi::OsStrExt;
                path.as_os_str().as_bytes()
            };
            #[cfg(not(unix))]
            let p = path.to_str().ok_or(Utf8Error)?.as_bytes();
            p
        }),
    };
    Ok(utf8_bytes)
}

/// Similar to [`into_bytes()`] but panics if malformed surrogates are encountered on windows.
pub fn into_bytes_or_panic_on_windows<'a>(path: impl Into<Cow<'a, Path>>) -> Cow<'a, [u8]> {
    into_bytes(path).expect("prefix path doesn't contain ill-formed UTF-8")
}

/// Given `input` bytes, produce a `Path` from them ignoring encoding entirely if on unix.
///
/// On windows, the input is required to be valid UTF-8, which is guaranteed if we wrote it before. There are some potential
/// git versions and windows installation which produce mal-formed UTF-16 if certain emojies are in the path. It's as rare as
/// it sounds, but possible.
pub fn from_byte_slice(input: &[u8]) -> Result<&Path, Utf8Error> {
    #[cfg(unix)]
    let p = {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(input).as_ref()
    };
    #[cfg(not(unix))]
    let p = Path::new(std::str::from_utf8(input).map_err(|_| Utf8Error)?);
    Ok(p)
}

/// Similar to [`from_byte_slice()`], but takes either borrowed or owned `input`.
pub fn from_bytes<'a>(input: impl Into<Cow<'a, [u8]>>) -> Result<Cow<'a, Path>, Utf8Error> {
    let input = input.into();
    match input {
        Cow::Borrowed(input) => from_byte_slice(input).map(Cow::Borrowed),
        Cow::Owned(input) => from_byte_vec(input).map(Cow::Owned),
    }
}

/// Similar to [`from_byte_slice()`], but takes either borrowed or owned `input`.
pub fn from_bytes_or_panic_on_windows<'a>(input: impl Into<Cow<'a, [u8]>>) -> Cow<'a, Path> {
    from_bytes(input).expect("prefix path doesn't contain ill-formed UTF-8")
}

/// Similar to [`from_byte_slice()`], but takes either borrowed or owned `input` as bstr.
#[cfg(feature = "bstr")]
pub fn from_bstr<'a>(input: impl Into<Cow<'a, bstr::BStr>>) -> Result<Cow<'a, Path>, Utf8Error> {
    let input = input.into();
    match input {
        Cow::Borrowed(input) => from_byte_slice(input).map(Cow::Borrowed),
        Cow::Owned(input) => from_byte_vec(input).map(Cow::Owned),
    }
}

/// Similar to [`from_byte_slice()`], but takes and produces owned data.
pub fn from_byte_vec(input: impl Into<Vec<u8>>) -> Result<PathBuf, Utf8Error> {
    let input = input.into();
    #[cfg(unix)]
    let p = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(input).into()
    };
    #[cfg(not(unix))]
    let p = PathBuf::from(String::from_utf8(input).map_err(|_| Utf8Error)?);
    Ok(p)
}

/// Similar to [`from_byte_vec()`], but will panic if there is ill-formed UTF-8 in the `input`.
pub fn from_byte_vec_or_panic_on_windows(input: impl Into<Vec<u8>>) -> PathBuf {
    from_byte_vec(input).expect("well-formed UTF-8 on windows")
}

/// Similar to [`from_byte_slice()`], but will panic if there is ill-formed UTF-8 in the `input`.
pub fn from_byte_slice_or_panic_on_windows(input: &[u8]) -> &Path {
    from_byte_slice(input).expect("well-formed UTF-8 on windows")
}

fn replace<'a>(path: impl Into<Cow<'a, [u8]>>, find: u8, replace: u8) -> Cow<'a, [u8]> {
    let path = path.into();
    match path {
        Cow::Owned(mut path) => {
            for b in path.iter_mut().filter(|b| **b == find) {
                *b = replace;
            }
            path.into()
        }
        Cow::Borrowed(path) => {
            if !path.contains(&find) {
                return path.into();
            }
            let mut path = path.to_owned();
            for b in path.iter_mut().filter(|b| **b == find) {
                *b = replace;
            }
            path.into()
        }
    }
}

/// Assures the given bytes use the native path separator.
pub fn to_native_separators<'a>(path: impl Into<Cow<'a, [u8]>>) -> Cow<'a, [u8]> {
    #[cfg(not(windows))]
    let p = to_unix_separators(path);
    #[cfg(windows)]
    let p = to_windows_separators(path);
    p
}

/// Convert paths with slashes to backslashes on windows and do nothing on unix. Takes a Cow as input
pub fn to_windows_separators_on_windows_or_panic<'a>(path: impl Into<Cow<'a, [u8]>>) -> Cow<'a, std::path::Path> {
    #[cfg(not(windows))]
    {
        crate::from_bytes_or_panic_on_windows(path)
    }
    #[cfg(windows)]
    {
        crate::from_bytes_or_panic_on_windows(to_windows_separators(path))
    }
}

/// Replaces windows path separators with slashes, but only do so on windows.
pub fn to_unix_separators_on_windows<'a>(path: impl Into<Cow<'a, [u8]>>) -> Cow<'a, [u8]> {
    #[cfg(windows)]
    {
        replace(path, b'\\', b'/')
    }
    #[cfg(not(windows))]
    {
        path.into()
    }
}

/// Replaces windows path separators with slashes.
///
/// **Note** Do not use these and prefer the conditional versions of this method.
pub fn to_unix_separators<'a>(path: impl Into<Cow<'a, [u8]>>) -> Cow<'a, [u8]> {
    replace(path, b'\\', b'/')
}

/// Find backslashes and replace them with slashes, which typically resembles a unix path.
///
/// **Note** Do not use these and prefer the conditional versions of this method.
pub fn to_windows_separators<'a>(path: impl Into<Cow<'a, [u8]>>) -> Cow<'a, [u8]> {
    replace(path, b'/', b'\\')
}

/// Obtain a `BStr` compatible `Cow` from one that is bytes.
pub fn into_bstr(path: Cow<'_, [u8]>) -> Cow<'_, bstr::BStr> {
    match path {
        Cow::Owned(p) => Cow::Owned(p.into()),
        Cow::Borrowed(p) => Cow::Borrowed(p.into()),
    }
}