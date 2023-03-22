macro_rules! invalid_data {
    ($e:expr) => {
        Err(::std::io::Error::new(::std::io::ErrorKind::InvalidData, $e))
    };
    ($fmt:expr, $($arg:tt)+) => {
        Err(::std::io::Error::new(::std::io::ErrorKind::InvalidData, format!($fmt, $($arg)+)))
    };
}

macro_rules! invalid_input {
    ($e:expr) => {
        Err(::std::io::Error::new(::std::io::ErrorKind::InvalidInput, $e))
    };
    ($fmt:expr, $($arg:tt)+) => {
        Err(::std::io::Error::new(::std::io::ErrorKind::InvalidInput, format!($fmt, $($arg)+)))
    };
}
