//! One-operation connection grammar. This module does not authenticate TLS or
//! authorize grants. Callers must do both and compare the frozen local domain
//! before restoring values; the sequence checker only enforces wire correlation.
use crate::{
    Error, Result,
    bounds::{self, cap, require},
    query::Query,
    value::Value,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

pub const VERSION: &str = "gaze-lens-source/2";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Query,
    Schema,
    ListTables,
    LogTail,
    LogWindow,
    LogRegex,
    Inspect,
    Readiness,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Version {
    #[serde(rename = "gaze-lens-source/2")]
    V2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Privacy {
    #[serde(rename = "client_gaze")]
    ClientGaze,
}
crate::closed_object! {
#[derive(Clone, PartialEq, Eq)]
pub struct DestinationBinding {
    #[serde(deserialize_with = "opaque")]
    pub principal: String,
    #[serde(deserialize_with = "opaque")]
    pub principal_generation: String,
    #[serde(deserialize_with = "opaque")]
    pub resource: String,
    #[serde(deserialize_with = "opaque")]
    pub resource_generation: String,
}
}
fn opaque<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    struct Opaque;
    impl serde::de::Visitor<'_> for Opaque {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("opaque binding")
        }
        fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<String, E> {
            if s.len() == 32
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                Ok(s.to_owned())
            } else {
                Err(E::custom("invalid_request"))
            }
        }
    }
    d.deserialize_str(Opaque)
}
impl DestinationBinding {
    pub fn validate(&self) -> Result<()> {
        for field in [
            &self.principal,
            &self.principal_generation,
            &self.resource,
            &self.resource_generation,
        ] {
            require(
                field.len() == 32
                    && field
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            )?;
        }
        Ok(())
    }
}
crate::closed_object! {
pub struct Prepare {
    pub version: Version,
    pub privacy: Privacy,
    pub id: String,
    pub credential: String,
    pub resource: String,
    pub operation: Operation,
}
}
crate::closed_object! {
pub struct Prepared {
    pub version: Version,
    pub privacy: Privacy,
    pub id: String,
    pub operation: Operation,
    pub binding: DestinationBinding,
}
}
crate::closed_object! {
pub struct Failure {
    pub version: Version,
    pub id: String,
    pub code: Error,
}
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCall<'a> {
    version: Version,
    id: String,
    operation: Operation,
    binding: DestinationBinding,
    #[serde(borrow)]
    args: &'a RawValue,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSuccess<'a> {
    version: Version,
    id: String,
    operation: Operation,
    binding: DestinationBinding,
    #[serde(borrow)]
    result: &'a RawValue,
}

pub struct Call {
    pub id: String,
    pub binding: DestinationBinding,
    pub args: Args,
}
pub struct Success {
    pub id: String,
    pub operation: Operation,
    pub binding: DestinationBinding,
    pub result: ResultBody,
}
crate::closed_object! {
pub struct Empty {}
}
crate::closed_object! {
pub struct Schema {
    pub table: String,
}
}
crate::closed_object! {
pub struct Tail {
    pub lines: usize,
}
}
crate::closed_object! {
pub struct Regex {
    pub pattern: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "level"
    )]
    pub level: Option<String>,
    pub limit: usize,
}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    Host,
    Packages,
    Php,
}
crate::closed_object! {
pub struct Inspect {
    pub view: View,
    pub collector: String,
    pub limit: usize,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "offset"
    )]
    pub offset: Option<usize>,
}
}
fn offset<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<usize>, D::Error> {
    usize::deserialize(d).map(Some)
}
fn level<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}
pub enum Args {
    Query(Query),
    Schema(Schema),
    ListTables(Empty),
    LogTail(Tail),
    LogWindow(Empty),
    LogRegex(Regex),
    Inspect(Inspect),
    Readiness(Empty),
}
impl Args {
    pub fn operation(&self) -> Operation {
        match self {
            Self::Query(_) => Operation::Query,
            Self::Schema(_) => Operation::Schema,
            Self::ListTables(_) => Operation::ListTables,
            Self::LogTail(_) => Operation::LogTail,
            Self::LogWindow(_) => Operation::LogWindow,
            Self::LogRegex(_) => Operation::LogRegex,
            Self::Inspect(_) => Operation::Inspect,
            Self::Readiness(_) => Operation::Readiness,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Query(q) => q.validate(),
            Self::Schema(s) => bounds::identifier(&s.table),
            Self::LogTail(t) => {
                require(t.lines > 0)?;
                cap(t.lines, bounds::KEYWORD_WINDOW_LINES)
            }
            Self::LogRegex(r) => {
                require(r.limit > 0)?;
                cap(r.limit, 1000)?;
                cap(r.pattern.len(), bounds::REGEX_BYTES)?;
                if let Some(l) = &r.level {
                    bounds::text(l)?;
                }
                Ok(())
            }
            Self::Inspect(i) => {
                bounds::configured_id(&i.collector)?;
                require(i.limit > 0)?;
                match i.view {
                    View::Host => require(i.limit == 1 && i.offset.is_none()),
                    View::Php => {
                        require(i.offset.is_none())?;
                        cap(i.limit, bounds::MAX_PHP)
                    }
                    View::Packages => {
                        cap(i.limit, bounds::MAX_PACKAGES)?;
                        cap(i.offset.unwrap_or(0), bounds::INVENTORY_RECORDS)
                    }
                }
            }
            _ => Ok(()),
        }
    }
    fn raw(&self) -> Result<Box<RawValue>> {
        match self {
            Self::Query(v) => raw(v, bounds::REQUEST_BYTES),
            Self::Schema(v) => raw(v, bounds::REQUEST_BYTES),
            Self::ListTables(v) | Self::LogWindow(v) | Self::Readiness(v) => {
                raw(v, bounds::REQUEST_BYTES)
            }
            Self::LogTail(v) => raw(v, bounds::REQUEST_BYTES),
            Self::LogRegex(v) => raw(v, bounds::REQUEST_BYTES),
            Self::Inspect(v) => raw(v, bounds::REQUEST_BYTES),
        }
    }
}
fn raw<T: Serialize>(v: &T, max: usize) -> Result<Box<RawValue>> {
    bounds::serialized_size(v, max)?;
    serde_json::value::to_raw_value(v).map_err(|_| Error::InvalidRequest)
}
fn parse<'a, T: Deserialize<'a>>(s: &'a str) -> Result<T> {
    serde_json::from_str(s).map_err(|_| Error::InvalidRequest)
}
fn args(op: Operation, s: &str) -> Result<Args> {
    let args = match op {
        Operation::Query => Args::Query(parse(s)?),
        Operation::Schema => Args::Schema(parse(s)?),
        Operation::ListTables => Args::ListTables(parse(s)?),
        Operation::LogTail => Args::LogTail(parse(s)?),
        Operation::LogWindow => Args::LogWindow(parse(s)?),
        Operation::LogRegex => Args::LogRegex(parse(s)?),
        Operation::Inspect => Args::Inspect(parse(s)?),
        Operation::Readiness => Args::Readiness(parse(s)?),
    };
    args.validate()?;
    Ok(args)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Bytes,
    Rows,
    Lines,
    ScanBytes,
    ScanLines,
    Boundary,
    Records,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    #[serde(rename = "tail_window")]
    TailWindow,
}
crate::closed_object! {
pub struct Window {
    pub scope: Scope,
    pub scanned_bytes: usize,
    pub scanned_lines: usize,
    pub admitted_lines: usize,
}
}
crate::closed_object! {
pub struct PackagePage {
    pub offset: usize,
    pub returned: usize,
    pub total: usize,
    #[serde(deserialize_with = "nullable")]
    pub next_offset: Option<usize>,
}
}
fn nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
crate::closed_object! {
pub struct Column {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Unavailable,
    Unsupported,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    OsRelease,
    DpkgDatabase,
    ConfiguredPhp,
    None,
}
crate::closed_object! {
pub struct Host {
    pub os_id: String,
    #[serde(deserialize_with = "nullable")]
    pub version_id: Option<String>,
    pub architecture: String,
}
}
crate::closed_object! {
pub struct Package {
    pub name: String,
    pub version: String,
    pub architecture: String,
}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageStatus {
    Present,
    Absent,
    Unavailable,
    Unsupported,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    Observed,
    Unavailable,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppBinding {
    Proven,
    Unknown,
}
crate::closed_object! {
pub struct PackageObservation {
    pub status: PackageStatus,
    #[serde(deserialize_with = "nullable")]
    pub name: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub version: Option<String>,
}
}
crate::closed_object! {
pub struct CliObservation {
    pub status: ObservationStatus,
    #[serde(deserialize_with = "nullable")]
    pub version: Option<String>,
}
}
crate::closed_object! {
pub struct FpmObservation {
    pub status: ObservationStatus,
    #[serde(deserialize_with = "nullable")]
    pub version: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub active: Option<bool>,
    pub app_binding: AppBinding,
}
}
crate::closed_object! {
pub struct Php {
    pub runtime_id: String,
    pub installed_package: PackageObservation,
    pub cli: CliObservation,
    pub fpm: FpmObservation,
}
}
#[derive(Serialize)]
#[serde(untagged)]
pub enum Records {
    Host(Vec<Host>),
    Packages(Vec<Package>),
    Php(Vec<Php>),
}
impl Records {
    pub fn len(&self) -> usize {
        match self {
            Self::Host(v) => v.len(),
            Self::Packages(v) => v.len(),
            Self::Php(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadinessStatus {
    #[serde(rename = "configured")]
    Configured,
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResultBody {
    QueryRows {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
        truncated: Vec<Reason>,
    },
    TableSchema {
        table: String,
        columns: Vec<Column>,
        truncated: Vec<Reason>,
    },
    TableList {
        tables: Vec<String>,
        truncated: Vec<Reason>,
    },
    LogWindow {
        lines: Vec<String>,
        window: Window,
        truncated: Vec<Reason>,
    },
    LogMatches {
        lines: Vec<String>,
        window: Window,
        matched_lines: usize,
        truncated: Vec<Reason>,
    },
    Inspection {
        view: View,
        collector: String,
        status: Status,
        evidence: Evidence,
        records: Records,
        page: Option<PackagePage>,
        truncated: Vec<Reason>,
    },
    Readiness {
        status: ReadinessStatus,
    },
}
fn result(s: &str) -> Result<ResultBody> {
    require(s.starts_with('{'))?;
    #[derive(Deserialize)]
    struct Tag {
        kind: String,
    }
    let tag: Tag = parse(s)?;
    macro_rules! fields {($($f:ident:$t:ty),* $(,)?)=>{{#[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Fields {#[serde(rename="kind")]_kind:String,$($f:$t),*}parse::<Fields>(s)?}};}
    Ok(match tag.kind.as_str() {
        "query_rows" => {
            let f = fields!(columns:Vec<String>,rows:Vec<Vec<Value>>,truncated:Vec<Reason>);
            ResultBody::QueryRows {
                columns: f.columns,
                rows: f.rows,
                truncated: f.truncated,
            }
        }
        "table_schema" => {
            let f = fields!(table:String,columns:Vec<Column>,truncated:Vec<Reason>);
            ResultBody::TableSchema {
                table: f.table,
                columns: f.columns,
                truncated: f.truncated,
            }
        }
        "table_list" => {
            let f = fields!(tables:Vec<String>,truncated:Vec<Reason>);
            ResultBody::TableList {
                tables: f.tables,
                truncated: f.truncated,
            }
        }
        "log_window" => {
            let f = fields!(lines:Vec<String>,window:Window,truncated:Vec<Reason>);
            ResultBody::LogWindow {
                lines: f.lines,
                window: f.window,
                truncated: f.truncated,
            }
        }
        "log_matches" => {
            let f =
                fields!(lines:Vec<String>,window:Window,matched_lines:usize,truncated:Vec<Reason>);
            ResultBody::LogMatches {
                lines: f.lines,
                window: f.window,
                matched_lines: f.matched_lines,
                truncated: f.truncated,
            }
        }
        "inspection" => {
            // Raw page preserves the distinction between required null and missing.
            let f = fields!(view:View,collector:String,status:Status,evidence:Evidence,records:Box<RawValue>,page:Box<RawValue>,truncated:Vec<Reason>);
            let records = match f.view {
                View::Host => Records::Host(parse(f.records.get())?),
                View::Packages => Records::Packages(parse(f.records.get())?),
                View::Php => Records::Php(parse(f.records.get())?),
            };
            ResultBody::Inspection {
                view: f.view,
                collector: f.collector,
                status: f.status,
                evidence: f.evidence,
                records,
                page: parse(f.page.get())?,
                truncated: f.truncated,
            }
        }
        "readiness" => {
            let f = fields!(status:ReadinessStatus);
            ResultBody::Readiness { status: f.status }
        }
        _ => return Err(Error::InvalidRequest),
    })
}
fn reasons(xs: &[Reason], allowed: &[Reason]) -> Result<()> {
    require(xs.windows(2).all(|w| w[0] < w[1]) && xs.iter().all(|r| allowed.contains(r)))
}
fn labels(xs: &[String], max: usize, sorted: bool) -> Result<()> {
    cap(xs.len(), max)?;
    let mut seen = std::collections::BTreeSet::new();
    for x in xs {
        bounds::identifier(x)?;
        require(seen.insert(x))?;
    }
    require(!sorted || xs.windows(2).all(|w| w[0] < w[1]))
}
fn log_window(w: &Window, lines: &[String], truncated: &[Reason]) -> Result<()> {
    cap(w.scanned_bytes, bounds::SCAN_BYTES)?;
    cap(w.scanned_lines, bounds::SCAN_LINES)?;
    require(w.admitted_lines <= w.scanned_lines && w.scanned_lines <= w.scanned_bytes)?;
    let mut bytes = 0usize;
    for line in lines {
        bounds::text(line)?;
        require(!line.contains('\n'))?;
        bytes = bytes
            .checked_add(line.len() + 1)
            .ok_or(Error::CapExceeded)?;
    }
    require(bytes <= w.scanned_bytes)?;
    reasons(
        truncated,
        &[
            Reason::Bytes,
            Reason::Lines,
            Reason::ScanBytes,
            Reason::ScanLines,
            Reason::Boundary,
        ],
    )?;
    // Effective acquisition ceilings belong to trusted source policy, not this DTO.
    Ok(())
}
impl ResultBody {
    pub fn validate_for(&self, args: &Args) -> Result<()> {
        args.validate()?;
        bounds::serialized_size(self, bounds::RESULT_BYTES)?;
        match (self, args) {
            (
                Self::QueryRows {
                    columns,
                    rows,
                    truncated,
                },
                Args::Query(q),
            ) => {
                require(!columns.is_empty())?;
                labels(columns, bounds::MAX_COLUMNS, false)?;
                cap(rows.len(), q.limit())?;
                for row in rows {
                    require(row.len() == columns.len())?;
                    for value in row {
                        value.validate()?;
                    }
                }
                reasons(truncated, &[Reason::Bytes, Reason::Rows])
            }
            (
                Self::TableSchema {
                    table,
                    columns,
                    truncated,
                },
                Args::Schema(_),
            ) => {
                bounds::identifier(table)?;
                cap(columns.len(), bounds::MAX_ENTRIES)?;
                let mut last = None;
                for c in columns {
                    bounds::identifier(&c.name)?;
                    bounds::text(&c.data_type)?;
                    require(last.is_none_or(|s: &str| s < c.name.as_str()))?;
                    last = Some(c.name.as_str());
                }
                reasons(truncated, &[Reason::Bytes, Reason::Records])
            }
            (Self::TableList { tables, truncated }, Args::ListTables(_)) => {
                labels(tables, bounds::MAX_ENTRIES, true)?;
                reasons(truncated, &[Reason::Bytes, Reason::Records])
            }
            (
                Self::LogWindow {
                    lines,
                    window,
                    truncated,
                },
                Args::LogTail(_) | Args::LogWindow(_),
            ) => {
                let limit = if let Args::LogTail(t) = args {
                    t.lines
                } else {
                    bounds::KEYWORD_WINDOW_LINES
                };
                cap(lines.len(), limit)?;
                require(window.admitted_lines == lines.len())?;
                log_window(window, lines, truncated)
            }
            (
                Self::LogMatches {
                    lines,
                    window,
                    matched_lines,
                    truncated,
                },
                Args::LogRegex(r),
            ) => {
                cap(lines.len(), r.limit)?;
                require(lines.len() <= *matched_lines && *matched_lines <= window.admitted_lines)?;
                log_window(window, lines, truncated)?;
                require(!truncated.contains(&Reason::Lines) || *matched_lines > lines.len())
            }
            (
                Self::Inspection {
                    view,
                    collector,
                    status,
                    evidence,
                    records,
                    page,
                    truncated,
                },
                Args::Inspect(i),
            ) => {
                require(*view == i.view && *collector == i.collector)?;
                bounds::configured_id(collector)?;
                require(matches!(
                    (view, records),
                    (View::Host, Records::Host(_))
                        | (View::Packages, Records::Packages(_))
                        | (View::Php, Records::Php(_))
                ))?;
                if *status != Status::Ok {
                    let expected = match view {
                        View::Host => Evidence::OsRelease,
                        View::Packages => Evidence::DpkgDatabase,
                        View::Php => Evidence::ConfiguredPhp,
                    };
                    require(*evidence == Evidence::None || *evidence == expected)?;
                    require(records.is_empty() && page.is_none() && truncated.is_empty())?;
                    return Ok(());
                }
                cap(records.len(), i.limit)?;
                match records {
                    Records::Host(xs) => {
                        require(
                            *evidence == Evidence::OsRelease
                                && xs.len() == 1
                                && page.is_none()
                                && truncated.is_empty(),
                        )?;
                        for x in xs {
                            bounds::text(&x.os_id)?;
                            bounds::text(&x.architecture)?;
                            if let Some(v) = &x.version_id {
                                bounds::text(v)?;
                            }
                        }
                    }
                    Records::Packages(xs) => {
                        require(*evidence == Evidence::DpkgDatabase)?;
                        reasons(truncated, &[Reason::Bytes, Reason::Records])?;
                        let mut last = None;
                        for x in xs {
                            for s in [&x.name, &x.version, &x.architecture] {
                                bounds::text(s)?;
                            }
                            let key = (x.name.as_str(), x.architecture.as_str());
                            require(last.is_none_or(|p| p < key))?;
                            last = Some(key);
                        }
                        let p = page.as_ref().ok_or(Error::InvalidRequest)?;
                        p.validate(i, xs.len(), truncated)?;
                    }
                    Records::Php(xs) => {
                        require(*evidence == Evidence::ConfiguredPhp && page.is_none())?;
                        reasons(truncated, &[Reason::Bytes, Reason::Records])?;
                        let mut last = None;
                        for x in xs {
                            bounds::configured_id(&x.runtime_id)?;
                            require(last.is_none_or(|p: &str| p < x.runtime_id.as_str()))?;
                            last = Some(x.runtime_id.as_str());
                            let p = &x.installed_package;
                            require(
                                (p.status == PackageStatus::Present) == p.version.is_some()
                                    && (p.status == PackageStatus::Present) == p.name.is_some(),
                            )?;
                            require(
                                (x.cli.status == ObservationStatus::Observed)
                                    == x.cli.version.is_some(),
                            )?;
                            require(
                                (x.fpm.status == ObservationStatus::Observed)
                                    == x.fpm.version.is_some(),
                            )?;
                            if x.fpm.status != ObservationStatus::Observed {
                                require(
                                    x.fpm.active.is_none()
                                        && x.fpm.app_binding == AppBinding::Unknown,
                                )?;
                            }
                            if x.fpm.app_binding == AppBinding::Proven {
                                require(x.fpm.active == Some(true))?;
                            }
                            for s in [&p.name, &p.version, &x.cli.version, &x.fpm.version]
                                .into_iter()
                                .flatten()
                            {
                                bounds::text(s)?;
                            }
                        }
                    }
                }
                Ok(())
            }
            (Self::Readiness { .. }, Args::Readiness(_)) => Ok(()),
            _ => Err(Error::InvalidRequest),
        }
    }
}
impl PackagePage {
    pub fn validate(&self, request: &Inspect, returned: usize, truncated: &[Reason]) -> Result<()> {
        require(
            request.view == View::Packages
                && self.offset == request.offset.unwrap_or(0)
                && self.returned == returned,
        )?;
        cap(self.offset, bounds::INVENTORY_RECORDS)?;
        cap(self.total, bounds::INVENTORY_RECORDS)?;
        require(request.limit > 0)?;
        cap(request.limit, bounds::MAX_PACKAGES)?;
        reasons(truncated, &[Reason::Bytes, Reason::Records])?;
        cap(returned, request.limit)?;
        let end = self
            .offset
            .checked_add(returned)
            .ok_or(Error::CapExceeded)?;
        if self.offset >= self.total {
            require(returned == 0 && self.next_offset.is_none() && truncated.is_empty())
        } else {
            require(returned > 0 && end <= self.total)?;
            require(self.next_offset == if end < self.total { Some(end) } else { None })?;
            if end < self.total {
                require(!truncated.is_empty())?;
            } else {
                require(truncated.is_empty())?;
            }
            require(!truncated.contains(&Reason::Records) || end < self.total)
        }
    }
}

fn id(id: &str) -> Result<()> {
    require(
        id.len() == 26
            && id.as_bytes()[0] <= b'7'
            && id
                .bytes()
                .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b)),
    )
}
fn frame(bytes: &[u8], max: usize) -> Result<&str> {
    cap(bytes.len(), max)?;
    require(bytes.last() == Some(&b'\n') && !bytes[..bytes.len() - 1].contains(&b'\n'))?;
    let body = &bytes[..bytes.len() - 1];
    bounds::frame_json(body, max - 1)?;
    require(body.first() == Some(&b'{') && body.last() == Some(&b'}'))?;
    std::str::from_utf8(body).map_err(|_| Error::InvalidRequest)
}
fn encode<T: Serialize>(v: &T, max: usize) -> Result<Vec<u8>> {
    let n = bounds::serialized_size(v, max - 1)?;
    let mut out = Vec::new();
    out.try_reserve_exact(n + 1)
        .map_err(|_| Error::CapExceeded)?;
    serde_json::to_writer(&mut out, v).map_err(|_| Error::InvalidRequest)?;
    out.push(b'\n');
    frame(&out, max)?;
    Ok(out)
}
pub fn decode_prepare(bytes: &[u8]) -> Result<Prepare> {
    let p: Prepare = parse(frame(bytes, bounds::PREPARE_BYTES)?)?;
    id(&p.id)?;
    bounds::configured_id(&p.resource)?;
    require(p.credential.len() == 64 && p.credential.bytes().all(|b| b.is_ascii_hexdigit()))?;
    Ok(p)
}
pub fn encode_prepare(p: &Prepare) -> Result<Vec<u8>> {
    let out = encode(p, bounds::PREPARE_BYTES)?;
    decode_prepare(&out)?;
    Ok(out)
}
pub fn decode_prepared(bytes: &[u8]) -> Result<Prepared> {
    let p: Prepared = parse(frame(bytes, bounds::PREPARE_BYTES)?)?;
    id(&p.id)?;
    p.binding.validate()?;
    Ok(p)
}
pub fn encode_prepared(p: &Prepared) -> Result<Vec<u8>> {
    let out = encode(p, bounds::PREPARE_BYTES)?;
    decode_prepared(&out)?;
    Ok(out)
}
pub fn decode_call(bytes: &[u8]) -> Result<Call> {
    let c: RawCall = parse(frame(bytes, bounds::FRAME_BYTES)?)?;
    id(&c.id)?;
    c.binding.validate()?;
    cap(c.args.get().len(), bounds::REQUEST_BYTES)?;
    Ok(Call {
        id: c.id,
        binding: c.binding,
        args: args(c.operation, c.args.get())?,
    })
}
pub fn encode_call(c: &Call) -> Result<Vec<u8>> {
    c.args.validate()?;
    let a = c.args.raw()?;
    cap(a.get().len(), bounds::REQUEST_BYTES)?;
    let raw = RawCall {
        version: Version::V2,
        id: c.id.clone(),
        operation: c.args.operation(),
        binding: c.binding.clone(),
        args: &a,
    };
    let out = encode(&raw, bounds::FRAME_BYTES)?;
    decode_call(&out)?;
    Ok(out)
}
pub fn decode_success(bytes: &[u8], call: &Call) -> Result<Success> {
    let r: RawSuccess = parse(frame(bytes, bounds::FRAME_BYTES)?)?;
    id(&r.id)?;
    r.binding.validate()?;
    require(r.id == call.id && r.operation == call.args.operation())?;
    if r.binding != call.binding {
        return Err(Error::BindingChanged);
    }
    cap(r.result.get().len(), bounds::RESULT_BYTES)?;
    let result = result(r.result.get())?;
    result.validate_for(&call.args)?;
    Ok(Success {
        id: r.id,
        operation: r.operation,
        binding: r.binding,
        result,
    })
}
pub fn encode_success(s: &Success, call: &Call) -> Result<Vec<u8>> {
    s.result.validate_for(&call.args)?;
    let result = raw(&s.result, bounds::RESULT_BYTES)?;
    let raw = RawSuccess {
        version: Version::V2,
        id: s.id.clone(),
        operation: s.operation,
        binding: s.binding.clone(),
        result: &result,
    };
    let out = encode(&raw, bounds::FRAME_BYTES)?;
    decode_success(&out, call)?;
    Ok(out)
}
pub fn decode_failure(bytes: &[u8], expected_id: &str) -> Result<Failure> {
    let f: Failure = parse(frame(bytes, bounds::PREPARE_BYTES)?)?;
    id(&f.id)?;
    require(f.id == expected_id)?;
    Ok(f)
}
pub fn encode_failure(f: &Failure) -> Result<Vec<u8>> {
    id(&f.id)?;
    encode(f, bounds::PREPARE_BYTES)
}

/// Pure connection-local sequence checking, never an authentication capability.
/// Any error permanently closes this instance. A reconnect requires a new one.
#[derive(Default)]
pub struct Sequence {
    state: State,
}
#[derive(Default)]
enum State {
    #[default]
    New,
    Preparing {
        id: String,
        operation: Operation,
    },
    Prepared {
        id: String,
        operation: Operation,
        binding: DestinationBinding,
    },
    Called(Call),
    Closed,
}
impl Sequence {
    pub fn prepare(&mut self, p: Prepare) -> Result<()> {
        let state = std::mem::replace(&mut self.state, State::Closed);
        require(matches!(state, State::New))?;
        encode_prepare(&p)?;
        self.state = State::Preparing {
            id: p.id,
            operation: p.operation,
        };
        Ok(())
    }
    pub fn prepared(&mut self, p: Prepared) -> Result<()> {
        let state = std::mem::replace(&mut self.state, State::Closed);
        let State::Preparing { id, operation } = state else {
            return Err(Error::InvalidRequest);
        };
        encode_prepared(&p)?;
        require(id == p.id && operation == p.operation)?;
        self.state = State::Prepared {
            id,
            operation,
            binding: p.binding,
        };
        Ok(())
    }
    pub fn call(&mut self, c: Call) -> Result<()> {
        let state = std::mem::replace(&mut self.state, State::Closed);
        let State::Prepared {
            id,
            operation,
            binding,
        } = state
        else {
            return Err(Error::InvalidRequest);
        };
        require(c.id == id && c.args.operation() == operation)?;
        if c.binding != binding {
            return Err(Error::BindingChanged);
        }
        encode_call(&c)?;
        self.state = State::Called(c);
        Ok(())
    }
    pub fn success(&mut self, bytes: &[u8]) -> Result<Success> {
        let state = std::mem::replace(&mut self.state, State::Closed);
        let State::Called(call) = state else {
            return Err(Error::InvalidRequest);
        };
        decode_success(bytes, &call)
    }
    /// Only after the caller authenticates Prepare may a correlated failure be used.
    pub fn failure(&mut self, bytes: &[u8]) -> Result<Failure> {
        let state = std::mem::replace(&mut self.state, State::Closed);
        let id = match &state {
            State::Preparing { id, .. } | State::Prepared { id, .. } => id,
            State::Called(c) => &c.id,
            _ => return Err(Error::InvalidRequest),
        };
        decode_failure(bytes, id)
    }
}
