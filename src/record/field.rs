use std::fmt;
use std::io::{Read, Write, Seek, SeekFrom};


use std::str::FromStr;

use byteorder::{LittleEndian, BigEndian, ReadBytesExt, WriteBytesExt};

use record::FieldInfo;
use Error;
use std::convert::TryFrom;
use writing::WriteableDbaseField;

#[derive(PartialEq, Copy, Clone)]
pub(crate) enum MemoFileType {
    DbaseMemo,
    DbaseMemo4,
    FoxBaseMemo,
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct MemoHeader {
    next_available_block_index: u32,
    block_size: u32,
}

impl MemoHeader {
    pub(crate) fn read_from<R: Read>(src: &mut R, memo_type: MemoFileType) -> std::io::Result<Self> {
        let next_available_block_index = src.read_u32::<LittleEndian>()?;
        let block_size = match memo_type {
            MemoFileType::DbaseMemo | MemoFileType::DbaseMemo4 => {
                match src.read_u16::<LittleEndian>()? {
                    0 => 512,
                    v => u32::from(v)
                }
            }
            MemoFileType::FoxBaseMemo => {
                let _ = src.read_u16::<BigEndian>();
                u32::from(src.read_u16::<BigEndian>()?)
            }
        };

        Ok(Self {
            next_available_block_index,
            block_size,
        })
    }
}

pub(crate) struct MemoReader<T: Read + Seek> {
    memo_file_type: MemoFileType,
    header: MemoHeader,
    source: T,
    internal_buffer: Vec<u8>,
}

impl<T: Read + Seek> MemoReader<T> {
    pub(crate) fn new(memo_type: MemoFileType, mut src: T) -> std::io::Result<Self> {
        let header = MemoHeader::read_from(&mut src, memo_type)?;
        let internal_buffer = vec![0u8; header.block_size as usize];
        Ok(Self {
            memo_file_type: memo_type,
            header,
            source: src,
            internal_buffer,
        })
    }

    fn read_data_at(&mut self, index: u32) -> std::io::Result<&[u8]> {
        let byte_offset = index * u32::from(self.header.block_size);
        self.source.seek(SeekFrom::Start(u64::from(byte_offset)))?;

        match self.memo_file_type {
            MemoFileType::FoxBaseMemo => {
                let _type = self.source.read_u32::<BigEndian>()?;
                let length = self.source.read_u32::<BigEndian>()?;
                if length as usize > self.internal_buffer.len() {
                    self.internal_buffer.resize(length as usize, 0);
                }
                let buf_slice = &mut self.internal_buffer[..length as usize];
                self.source.read_exact(buf_slice)?;
                match buf_slice.iter().rposition(|b| *b != 0) {
                    Some(pos) => {
                        Ok(&buf_slice[..pos + 1])
                    }
                    None => {
                        if buf_slice.iter().all(|b| *b == 0) {
                            Ok(&buf_slice[..0])
                        } else {
                            Ok(buf_slice)
                        }
                    }
                }
            }
            MemoFileType::DbaseMemo4 => {
                let _ = self.source.read_u32::<LittleEndian>()?;
                let length = self.source.read_u32::<LittleEndian>()?;
                self.source.read_exact(&mut self.internal_buffer[..length as usize])?;
                match self.internal_buffer[..length as usize].iter().position(|b| *b == 0x1F) {
                    Some(pos) => {
                        Ok(&self.internal_buffer[..pos])
                    }
                    None => {
                        Ok(&self.internal_buffer)
                    }
                }
            }
            MemoFileType::DbaseMemo => {
                if let Err(e) = self.source.read_exact(&mut self.internal_buffer) {
                    if index != self.header.next_available_block_index - 1 &&
                        e.kind() != std::io::ErrorKind::UnexpectedEof {
                        return Err(e);
                    }
                }
                match self.internal_buffer.iter().position(|b| *b == 0x1A) {
                    Some(pos) => {
                        Ok(&self.internal_buffer[..pos])
                    }
                    None => Ok(&self.internal_buffer)
                }
            }
        }
    }
}


#[derive(Debug, Copy, Clone, PartialEq)]
pub enum FieldType {
    // dBASE III
    Character,
    Date,
    Float,
    Numeric,
    Logical,
    // Visual FoxPro
    Currency,
    DateTime,
    Integer,
    // Unknown
    Double,
    Memo,
    //General,
    //BinaryCharacter,
    //BinaryMemo,
}

impl From<FieldType> for u8 {
    fn from(t: FieldType) -> Self {
        let v = match t {
            FieldType::Character => 'C',
            FieldType::Date => 'D',
            FieldType::Float => 'F',
            FieldType::Numeric => 'N',
            FieldType::Logical => 'L',
            FieldType::Currency => 'Y',
            FieldType::DateTime => 'T',
            FieldType::Integer => 'I',
            FieldType::Double => 'B',
            FieldType::Memo => 'M',
        };
        v as u8
    }
}

impl FieldType {
    pub fn from(c: char) -> Option<FieldType> {
        match c {
            // dBASE III field types
            // All stored as strings
            'C' => Some(FieldType::Character),
            'D' => Some(FieldType::Date),
            'F' => Some(FieldType::Float),
            'N' => Some(FieldType::Numeric),
            'L' => Some(FieldType::Logical),
            // Visual FoxPro field types
            // stored in binary formats
            'Y' => Some(FieldType::Currency),
            'T' => Some(FieldType::DateTime),
            'I' => Some(FieldType::Integer),
            // unknown version
            'B' => Some(FieldType::Double),
            'M' => Some(FieldType::Memo),
            //'G' => Some(FieldType::General),
            //'C' => Some(FieldType::BinaryCharacter), ??
            //'M' => Some(FieldType::BinaryMemo),
            _ => None,
        }
    }

    pub(crate) fn size(self) -> Option<u8> {
        match self {
            FieldType::Logical => Some(1),
            FieldType::Date => Some(8),
            FieldType::Integer => Some(std::mem::size_of::<i32>() as u8),
            FieldType::Currency => Some(std::mem::size_of::<f64>() as u8),
            FieldType::DateTime => Some(2 * std::mem::size_of::<i32>() as u8),
            FieldType::Double => Some(std::mem::size_of::<f64>() as u8),
            _ => None
        }
    }
    pub(crate) fn pad_byte(&self) -> Option<u8> {
        match *self {
            FieldType::Character |
            FieldType::Numeric |
            FieldType::Float |
            FieldType::Logical => Some(' ' as u8),
            _ => None
        }
    }
}

impl TryFrom<char> for FieldType {
    type Error = Error;

    fn try_from(c: char) -> Result<Self, Self::Error> {
        match Self::from(c) {
            Some(t) => Ok(t),
            None => Err(Error::InvalidFieldType(c)),
        }
    }
}


#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Date {
    year: u32,
    month: u32,
    day: u32,
}

#[cfg(feature = "serde")]
mod de {
    use super::*;
    use serde::de::{Deserialize, Visitor};
    use serde::Deserializer;

    impl<'de> Deserialize<'de> for Date {
        fn deserialize<D>(deserializer: D) -> Result<Self, <D as Deserializer<'de>>::Error> where
            D: Deserializer<'de> {
            struct DateVisitor;
            impl<'de> Visitor<'de> for DateVisitor {
                type Value = Date;

                fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                    formatter.write_str("struct Date")
                }

                fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E> where
                    E: serde::de::Error, {
                    let string = String::from_utf8(v).unwrap();
                    Ok(Date::from_str(&string).unwrap())
                }
            }
            deserializer.deserialize_byte_buf(DateVisitor)
        }
    }
}


#[cfg(feature = "serde")]
mod ser {
    use super::*;

    use serde::ser::Serialize;
    use serde::Serializer;

    impl Serialize for Date {
        fn serialize<S>(&self, serializer: S) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error> where
            S: Serializer {
            serializer.serialize_bytes(self.to_string().as_bytes())
        }
    }
}

impl Date {
    pub fn new(day: u32, month: u32, year: u32) -> Result<Self, Error> {
        if month > 12 || day > 31 || year < 1900 || year > 2155 {
            Err(Error::InvalidDate)
        } else {
            Ok(Self {
                year,
                month,
                day,
            })
        }
    }

    pub fn year(&self) -> u32 {
        self.year
    }

    pub fn month(&self) -> u32 {
        self.month
    }

    pub fn day(&self) -> u32 {
        self.day
    }

    pub(crate) fn from_bytes(bytes: [u8; 3]) -> Self {
        Self {
            year: 1900u32 + bytes[0] as u32,
            month: bytes[1] as u32,
            day: bytes[2] as u32,
        }
    }

    pub(crate) fn write_to<T: Write>(&self, dest: &mut T) -> Result<(), Error> {
        dest.write_u8((self.year - 1900) as u8)?;
        dest.write_u8(self.month as u8)?;
        dest.write_u8(self.day as u8)?;
        Ok(())
    }

    // https://en.wikipedia.org/wiki/Julian_day
    // at "Julian or Gregorian calendar from Julian day number"
    fn julian_day_number_to_gregorian_date(jdn: i32) -> Date {
        const Y: i32 = 4716;
        const J: i32 = 1401;
        const M: i32 = 2;
        const N: i32 = 12;
        const R: i32 = 4;
        const P: i32 = 1461;
        const V: i32 = 3;
        const U: i32 = 5;
        const S: i32 = 153;
        const W: i32 = 2;
        const B: i32 = 274277;
        const C: i32 = -38;


        let f = jdn + J + ((4 * jdn + B) / 146097 * 3) / 4 + C;
        let e = R * f + V;
        let g = (e % P) / R;
        let h = U * g + W;

        let day = (h % S) / U + 1;
        let month = ((h / S + M) % (N)) + 1;
        let year = (e / P) - Y + (N + M - month) / N;

        Date {
            year: year as u32,
            month: month as u32,
            day: day as u32,
        }
    }

    fn to_julian_day_number(&self) -> i32 {
        let (month, year) = if self.month > 2 {
            (self.month - 3, self.year)
        } else {
            (self.month + 9, self.year - 1)
        };

        let century = year / 100;
        let decade = year - 100 * century;

        ((146097 * century) / 4 + (1461 * decade) / 4 + (153 * month + 2) / 5 + self.day + 1721119) as i32
    }
}

impl FromStr for Date {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let year = s[0..4].parse::<u32>()?;
        let month = s[4..6].parse::<u32>()?;
        let day = s[6..8].parse::<u32>()?;

        Ok(Self { year, month, day })
    }
}

impl std::string::ToString for Date {
    fn to_string(&self) -> String {
        let mut s = String::with_capacity(8);
        let year_str = self.year.to_string();
        let month_str = self.month.to_string();
        let day_str = self.day.to_string();

        if self.year < 100 {
            s.push('0');
            s.push('0');
        } else if self.year < 1000 {
            s.push('0');
        }
        s.push_str(&year_str);

        if self.month < 10 {
            s.push('0');
        }
        s.push_str(&month_str);

        if self.day < 10 {
            s.push('0');
        }
        s.push_str(&day_str);
        s
    }
}

// TODO new() fn that validates inputs
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Time {
    hours: u32,
    minutes: u32,
    seconds: u32,
}

impl Time {
    const HOURS_FACTOR: i32 = 3_600_000;
    const MINUTES_FACTOR: i32 = 60_000;
    const SECONDS_FACTOR: i32 = 1_000;


    fn from_word(mut time_word: i32) -> Self {
        let hours: u32 = (time_word / Self::HOURS_FACTOR) as u32;
        time_word -= (hours * Self::HOURS_FACTOR as u32) as i32;
        let minutes: u32 = (time_word / Self::MINUTES_FACTOR) as u32;
        time_word -= (minutes * Self::MINUTES_FACTOR as u32) as i32;
        let seconds: u32 = (time_word / Self::SECONDS_FACTOR) as u32;
        Self {
            hours,
            minutes,
            seconds,
        }
    }

    fn to_time_word(&self) -> i32 {
        let mut time_word = self.hours * Self::HOURS_FACTOR as u32;
        time_word += self.minutes * Self::MINUTES_FACTOR as u32;
        time_word += self.seconds * Self::SECONDS_FACTOR as u32;
        time_word as i32
    }
}

// TODO new() fn that validates inputs
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct DateTime {
    date: Date,
    time: Time,
}

impl DateTime {
    fn read_from<T: Read>(src: &mut T) -> Result<Self, Error> {
        let julian_day_number = src.read_i32::<LittleEndian>()?;
        let time_word = src.read_i32::<LittleEndian>()?;
        let time = Time::from_word(time_word);
        let date = Date::julian_day_number_to_gregorian_date(julian_day_number);
        Ok(Self {
            date,
            time,
        })
    }

    fn write_to<W: Write>(&self, dest: &mut W) -> std::io::Result<()> {
        dest.write_i32::<LittleEndian>(self.date.to_julian_day_number())?;
        dest.write_i32::<LittleEndian>(self.time.to_time_word())?;
        Ok(())
    }
}



/// Enum where each variant stores the record value
#[derive(Debug, PartialEq)]
pub enum FieldValue {
    // dBase III fields
    // Stored as strings, fully padded (ie only space char) strings
    // are interpreted as None
    Character(Option<String>),
    Numeric(Option<f64>),
    Logical(Option<bool>),
    Date(Option<Date>),
    Float(Option<f32>),
    //Visual FoxPro fields
    Integer(i32),
    Currency(f64),
    DateTime(DateTime),
    Double(f64),
    Memo(String),
}

impl FieldValue {
    pub(crate) fn read_from<T: Read + Seek>(
        mut source: &mut T,
        memo_reader: &mut Option<MemoReader<T>>,
        field_info: &FieldInfo,
    ) -> Result<Self, Error> {
        let value = match field_info.field_type {
            FieldType::Logical => match source.read_u8()? as char {
                ' ' | '?' => FieldValue::Logical(None),
                '1' | '0' | 'T' | 't' | 'Y' | 'y' => FieldValue::Logical(Some(true)),
                'N' | 'n' | 'F' | 'f' => FieldValue::Logical(Some(false)),
                _ => FieldValue::Logical(None)
            },
            FieldType::Character => {
                let value = read_string_of_len(&mut source, field_info.field_length)?;
                let trimmed_value = value.trim();
                if trimmed_value.is_empty() {
                    FieldValue::Character(None)
                } else {
                    FieldValue::Character(Some(trimmed_value.to_owned()))
                }
            }
            FieldType::Numeric => {
                let value = read_string_of_len(&mut source, field_info.field_length)?;
                let trimmed_value = value.trim();
                if trimmed_value.is_empty() || value.chars().all(|c| c == '*') {
                    FieldValue::Numeric(None)
                } else {
                    FieldValue::Numeric(Some(trimmed_value.parse::<f64>()?))
                }
            }
            FieldType::Float => {
                let value = read_string_of_len(&mut source, field_info.field_length)?;
                let trimmed_value = value.trim();
                if trimmed_value.is_empty() || value.chars().all(|c| c == '*') {
                    FieldValue::Float(None)
                } else {
                    FieldValue::Float(Some(trimmed_value.parse::<f32>()?))
                }
            }
            FieldType::Date => {
                let value = read_string_of_len(&mut source, field_info.field_length)?;
                if value.chars().all(|c| c == ' ') {
                    FieldValue::Date(None)
                } else {
                    FieldValue::Date(Some(value.parse::<Date>()?))
                }
            }
            FieldType::Integer => FieldValue::Integer(source.read_i32::<LittleEndian>()?),
            FieldType::Double => FieldValue::Double(source.read_f64::<LittleEndian>()?),
            FieldType::Currency => FieldValue::Currency(source.read_f64::<LittleEndian>()?),
            FieldType::DateTime => FieldValue::DateTime(DateTime::read_from(&mut source)?),
            FieldType::Memo => {
                let index_in_memo =
                    if field_info.field_length > 4 {
                        let string = read_string_of_len(&mut source, field_info.field_length)?;
                        let trimmed_str = string.trim();
                        if trimmed_str.is_empty() {
                            return Ok(FieldValue::Memo(String::from("")));
                        } else {
                            trimmed_str.parse::<u32>()?
                        }
                    } else {
                        source.read_u32::<LittleEndian>()?
                    };

                if let Some(memo_reader) = memo_reader {
                    let data_from_memo = memo_reader.read_data_at(index_in_memo)?;
                    FieldValue::Memo(String::from_utf8_lossy(data_from_memo).to_string())
                } else {
                    return Err(Error::MissingMemoFile);
                }
            }
        };
        Ok(value)
    }

    pub fn field_type(&self) -> FieldType {
        match self {
            FieldValue::Character(_) => FieldType::Character,
            FieldValue::Numeric(_) => FieldType::Numeric,
            FieldValue::Logical(_) => FieldType::Logical,
            FieldValue::Integer(_) => FieldType::Integer,
            FieldValue::Float(_) => FieldType::Float,
            FieldValue::Double(_) => FieldType::Double,
            FieldValue::Date(_) => FieldType::Date,
            FieldValue::Memo(_) => FieldType::Memo,
            FieldValue::Currency(_) => FieldType::Currency,
            FieldValue::DateTime(_) => FieldType::DateTime
        }
    }
}

//TODO rename to WritableDBaseField ?
impl WriteableDbaseField for FieldValue {
    fn field_type(&self) -> FieldType {
        self.field_type()
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        match self {
            FieldValue::Character(value) => {
                if let Some(s) = value {
                    <&str as WriteableDbaseField>::write_to(&s.as_str(), dst)?;
                }
                Ok(())
            }
            FieldValue::Numeric(value) => {
                if let Some(n) = value {
                    <f64 as WriteableDbaseField>::write_to(n, dst)?;
                }
                Ok(())
            }
            FieldValue::Float(value) => {
                if let Some(n) = value {
                    write!(dst, "{}", n)?;
                }
                Ok(())
            }
            FieldValue::Logical(value) => {
                <Option<bool> as WriteableDbaseField>::write_to(value, dst)
            }
            FieldValue::Date(value) => {
                match value {
                    Some(d) => {
                        <Date as WriteableDbaseField>::write_to(&d, dst)?;
                    }
                    None => {
                        dst.write_all(&[' ' as u8; 8])?;
                    }
                }
                Ok(())
            }
            FieldValue::Double(d) => {
                dst.write_f64::<LittleEndian>(*d)?;
                Ok(())
            }
            FieldValue::Integer(i) => {
                dst.write_i32::<LittleEndian>(*i)
            }
            FieldValue::Memo(_text) => {
                unimplemented!();
            }
            FieldValue::Currency(c) => {
                dst.write_f64::<LittleEndian>(*c)
            }
            FieldValue::DateTime(dt) => {
                dt.write_to(dst)
            }
        }
    }
}

impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl From<&str> for FieldValue {
    fn from(s: &str) -> Self {
        FieldValue::Character(Some(String::from(s)))
    }
}

impl From<Date> for FieldValue {
    fn from(d: Date) -> Self {
        FieldValue::Date(Some(d))
    }
}

fn read_string_of_len<T: Read>(source: &mut T, len: u8) -> Result<String, std::io::Error> {
    let mut bytes = Vec::<u8>::new();
    bytes.resize(len as usize, 0u8);
    source.read_exact(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

impl WriteableDbaseField for f64 {
    fn field_type(&self) -> FieldType {
        FieldType::Numeric
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        write!(dst, "{}", self)
    }
}

impl WriteableDbaseField for Option<f64> {
    fn field_type(&self) -> FieldType {
        FieldType::Numeric
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        if let Some(f) = self {
            <f64 as WriteableDbaseField>::write_to(f, dst)
        } else {
            Ok(())
        }
    }
}

impl WriteableDbaseField for f32 {
    fn field_type(&self) -> FieldType {
        FieldType::Float
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        write!(dst, "{}", self)
    }
}

impl WriteableDbaseField for Option<f32> {
    fn field_type(&self) -> FieldType {
        FieldType::Float
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        if let Some(v) = self {
            <f32 as WriteableDbaseField>::write_to(v, dst)
        } else {
            Ok(())
        }
    }
}

impl WriteableDbaseField for &str {
    fn field_type(&self) -> FieldType {
        FieldType::Character
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        dst.write_all(self.as_bytes())
    }
}

impl WriteableDbaseField for String {
    fn field_type(&self) -> FieldType {
        FieldType::Character
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        self.as_str().write_to(dst)
    }
}

impl WriteableDbaseField for Option<String> {
    fn field_type(&self) -> FieldType {
        FieldType::Character
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        if let Some(s) = self {
            <String as WriteableDbaseField>::write_to(s, dst)
        } else {
            Ok(())
        }
    }
}

impl WriteableDbaseField for Date {
    fn field_type(&self) -> FieldType {
        FieldType::Date
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        write!(dst, "{:04}{:02}{:02}", self.year, self.month, self.day)
    }
}

impl WriteableDbaseField for Option<Date> {
    fn field_type(&self) -> FieldType {
        FieldType::Date
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        if let Some(d) = self {
            <Date as WriteableDbaseField>::write_to(d, dst)
        } else {
            Ok(())
        }
    }
}


impl WriteableDbaseField for bool {
    fn field_type(&self) -> FieldType {
        FieldType::Logical
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        if *self {
            write!(dst, "{}", 't')
        } else {
            write!(dst, "{}", 'f')
        }
    }
}

impl WriteableDbaseField for Option<bool> {
    fn field_type(&self) -> FieldType {
        FieldType::Logical
    }

    fn write_to<W: Write>(&self, dst: &mut W) -> std::io::Result<()> {
        if let Some(value) = self {
            value.write_to(dst)
        } else {
            write!(dst, "?")
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use record::FieldFlags;
    use std::io::{Cursor, Seek, SeekFrom};

    fn create_temp_record_field_info(field_type: FieldType, len: u8) -> FieldInfo {
        FieldInfo {
            name: "".to_owned(),
            field_type,
            displacement_field: [0u8; 4],
            field_length: len,
            num_decimal_places: 0,
            flags: FieldFlags { 0: 0u8 },
            autoincrement_next_val: [0u8; 5],
            autoincrement_step: 0u8,
        }
    }

    #[test]
    fn write_read_date() {
        let date = FieldValue::from(Date {
            year: 2019,
            month: 01,
            day: 01,
        });

        let mut out = Cursor::new(Vec::<u8>::new());
        date.write_to(&mut out).unwrap();
        assert_eq!(out.position(), u64::from(FieldType::Date.size().unwrap()));

        let record_info = create_temp_record_field_info(FieldType::Date, out.position() as u8);
        out.set_position(0);

        match FieldValue::read_from(&mut out, &mut None, &record_info).unwrap() {
            FieldValue::Date(Some(read_date)) => {
                assert_eq!(read_date.year, 2019);
                assert_eq!(read_date.month, 1);
                assert_eq!(read_date.day, 1);
            }
            _ => assert!(false, "Did not read a date ??"),
        }
    }

    #[test]
    fn test_write_read_empty_date() {
        let date = FieldValue::Date(None);

        let mut out = Cursor::new(Vec::<u8>::new());
        date.write_to(&mut out).unwrap();
        assert_eq!(out.position(), u64::from(FieldType::Date.size().unwrap()));

        let record_info = create_temp_record_field_info(FieldType::Date, out.position() as u8);
        out.set_position(0);


        match FieldValue::read_from(&mut out, &mut None, &record_info).unwrap() {
            FieldValue::Date(maybe_date) => assert!(maybe_date.is_none()),
            _ => assert!(false, "Did not read a date ??"),
        }
    }


    #[test]
    fn write_read_ascii_char() {
        let field = FieldValue::Character(Some(String::from("Only ASCII")));
        let mut out = Cursor::new(Vec::<u8>::new());
        field.write_to(&mut out).unwrap();

        let record_info =
            create_temp_record_field_info(FieldType::Character, out.position() as u8);
        out.set_position(0);


        match FieldValue::read_from(&mut out, &mut None, &record_info).unwrap() {
            FieldValue::Character(s) => {
                assert_eq!(s, Some(String::from("Only ASCII")));
            }
            _ => assert!(false, "Did not read a Character field ??"),
        }
    }


    #[test]
    fn write_read_utf8_char() {
        let field = FieldValue::Character(Some(String::from("🤔")));

        let mut out = Cursor::new(Vec::<u8>::new());
        field.write_to(&mut out).unwrap();

        let record_info =
            create_temp_record_field_info(FieldType::Character, out.position() as u8);
        out.set_position(0);


        match FieldValue::read_from(&mut out, &mut None, &record_info).unwrap() {
            FieldValue::Character(s) => {
                assert_eq!(s, Some(String::from("🤔")));
            }
            _ => assert!(false, "Did not read a Character field ??"),
        }
    }

    #[test]
    fn test_from_julian_day_number() {
        let date = Date::julian_day_number_to_gregorian_date(2458685);
        assert_eq!(date.year, 2019);
        assert_eq!(date.month, 07);
        assert_eq!(date.day, 20);
    }

    #[test]
    fn test_to_julian_day_number() {
        let date = Date { year: 2019, month: 07, day: 20 };
        assert_eq!(date.to_julian_day_number(), 2458685);
    }

    #[test]
    fn write_read_float() {
        let field = FieldValue::Float(Some(12.43));

        let mut out = Cursor::new(Vec::<u8>::new());
        field.write_to(&mut out).unwrap();

        let record_info =
            create_temp_record_field_info(FieldType::Float, out.position() as u8);
        out.set_position(0);


        match FieldValue::read_from(&mut out, &mut None, &record_info).unwrap() {
            FieldValue::Float(s) => {
                assert_eq!(s, Some(12.43));
            }
            _ => assert!(false, "Did not read a Float field ??"),
        }
    }
}
