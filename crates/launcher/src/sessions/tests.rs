// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{contract::testing::validate, testing::scratch};
use std::time::UNIX_EPOCH;

/// 2026-10-18T08:00:00Z.
fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_792_310_400)
}

fn id(c: char) -> String {
    c.to_string().repeat(32)
}

fn set_time(path: &Path, time: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

/// A session folder with a request written at `requested`.
fn session(data: &Path, id: &str, requested: SystemTime) -> PathBuf {
    let dir = paths::session_dir(data, id);
    std::fs::create_dir_all(&dir).unwrap();
    let request = serde_json::json!({
        "version": 1, "id": id, "action": "start", "game_directory": "game",
        "data_directory": data, "server_path": "server",
        "server_sha256": "e".repeat(64), "player_name": "Driver",
        "record_protocol": false
    });
    std::fs::write(dir.join("request.json"), request.to_string()).unwrap();
    set_time(&dir.join("request.json"), requested);
    dir
}

fn status(dir: &Path, id: &str, state: State, recorded: SystemTime) {
    let record = Status {
        version: records::VERSION,
        id: id.to_owned(),
        utc: clock::iso_utc(recorded),
        state,
        message: format!("{state} message"),
        worker_pid: 1,
        game_pid: None,
        server_pid: Some(2),
    };
    records::replace_json(&dir.join("status.json"), &record).unwrap();
    set_time(&dir.join("status.json"), recorded);
}

fn options() -> Options {
    let mut options = Options::new(DEFAULT_LIMIT, None);
    options.now = t0() + Duration::from_secs(3600);
    options
}

fn listed(data: &Path, options: &Options) -> SessionList {
    let list = list(data, options).unwrap();
    validate("sessions", &list);
    list
}

fn ids(list: &SessionList) -> Vec<String> {
    list.sessions.iter().map(|s| s.id.clone()).collect()
}

#[test]
fn an_absent_or_empty_data_directory_lists_nothing() {
    let base = scratch("sessions-empty");
    for data in [base.join("missing"), base.clone()] {
        let list = listed(&data, &options());
        assert!(list.sessions.is_empty() && !list.truncated && list.current.is_none());
        assert!(list.warnings.is_empty(), "{:?}", list.warnings);
        assert!(list.crash_dump_directory.is_none() && list.ea_desktop_logs.is_none());
    }
    assert!(!base.join("missing").exists(), "listing writes nothing");
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn sessions_are_newest_first_limited_and_filtered() {
    let data = scratch("sessions-order");
    for (n, c) in ['1', '2', '3'].into_iter().enumerate() {
        session(&data, &id(c), t0() + Duration::from_secs(10 * n as u64));
    }
    let launcher = data.join("artifacts").join("launcher");
    std::fs::create_dir(launcher.join("notes")).unwrap();
    std::fs::create_dir(launcher.join(id('A'))).unwrap();
    std::fs::write(launcher.join(id('4')), b"a file, not a session").unwrap();
    let list = listed(&data, &options());
    assert_eq!(ids(&list), [id('3'), id('2'), id('1')]);
    assert!(!list.truncated);
    assert_eq!(list.sessions[0].action, Some(Action::Start));
    assert_eq!(
        list.sessions[2].started_utc.as_deref(),
        Some("2026-10-18T08:00:00Z")
    );

    let mut limited = options();
    limited.limit = 2;
    let list = listed(&data, &limited);
    assert_eq!(ids(&list), [id('3'), id('2')]);
    assert!(list.truncated);
    limited.limit = 0;
    assert_eq!(listed(&data, &limited).sessions.len(), 1, "clamped to one");

    let mut one = options();
    one.id = Some(id('1'));
    assert_eq!(ids(&listed(&data, &one)), [id('1')]);
    one.id = Some(id('9'));
    assert!(listed(&data, &one).sessions.is_empty());

    // A folder without a request sorts by its own time and says why.
    std::fs::create_dir(launcher.join(id('5'))).unwrap();
    let list = listed(&data, &options());
    let bare = list.sessions.iter().find(|s| s.id == id('5')).unwrap();
    assert_eq!(list.sessions[0].id, id('5'), "created just now");
    assert!(bare.action.is_none() && bare.started_utc.is_none() && bare.state.is_none());
    assert_eq!(bare.warnings, ["request.json is missing."]);
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn states_come_from_status_and_dead_supervisors_read_as_interrupted() {
    let data = scratch("sessions-state");
    let at = |s: u64| t0() + Duration::from_secs(s);
    let finished = session(&data, &id('a'), at(0));
    status(&finished, &id('a'), State::Finished, at(5));
    let failed = session(&data, &id('b'), at(10));
    status(&failed, &id('b'), State::Failed, at(15));
    let orphan = session(&data, &id('c'), at(20));
    status(&orphan, &id('c'), State::Ready, at(25));
    let current = session(&data, &id('d'), at(30));
    status(&current, &id('d'), State::Ready, at(35));
    session::point_current_session(&data, &id('d')).unwrap();

    let list = listed(&data, &options());
    let get = |list: &SessionList, c| {
        list.sessions
            .iter()
            .find(|s| s.id == id(c))
            .cloned()
            .unwrap()
    };
    let a = get(&list, 'a');
    assert_eq!(a.state, Some(State::Finished));
    assert_eq!(a.ended_utc, a.updated_utc);
    assert_eq!(a.message.as_deref(), Some("finished message"));
    assert_eq!(get(&list, 'b').state, Some(State::Failed));
    let c = get(&list, 'c');
    assert_eq!(c.state, Some(State::Interrupted));
    assert!(c.ended_utc.is_none() && !c.current);
    assert!(c.message.unwrap().contains("no longer running"));
    let d = get(&list, 'd');
    assert!(d.current);
    assert_eq!(list.current, Some(id('d')));
    assert_eq!(d.state, Some(State::Interrupted), "no lock file at all");
    assert!(d.message.unwrap().contains("lock is missing"));

    let held = session::Lock::acquire(&data).unwrap();
    let list = listed(&data, &options());
    assert_eq!(get(&list, 'd').state, Some(State::Ready));
    assert_eq!(get(&list, 'c').state, Some(State::Interrupted));
    drop(held);

    // Damaged records become warnings on their own session only.
    std::fs::write(failed.join("status.json"), b"{\"version\":1,").unwrap();
    std::fs::write(
        orphan.join("status.json"),
        vec![b' '; records::MAX_RECORD_BYTES as usize + 1],
    )
    .unwrap();
    status(&finished, &id('e'), State::Finished, at(5));
    std::fs::write(current.join("request.json"), b"not json").unwrap();
    std::fs::write(
        data.join(session::POINTER_FILE),
        b"{\"version\":1,\"id\":\"x\"}",
    )
    .unwrap();
    let list = listed(&data, &options());
    assert_eq!(list.sessions.len(), 4);
    for c in ['a', 'b', 'c'] {
        let s = get(&list, c);
        assert!(s.state.is_none() && s.message.is_none(), "{c}");
        assert_eq!(s.warnings.len(), 1, "{c}: {:?}", s.warnings);
    }
    assert!(get(&list, 'a').warnings[0].contains("another session"));
    assert!(get(&list, 'd').action.is_none());
    assert!(list.current.is_none());
    assert_eq!(
        list.warnings,
        ["current-session.json is not a valid session pointer."]
    );
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn files_are_listed_with_line_counts_and_the_log_bound() {
    let data = scratch("sessions-files");
    let dir = session(&data, &id('f'), t0());
    std::fs::write(dir.join("a.log"), b"x\ny\n").unwrap();
    std::fs::write(dir.join("b.log"), b"x\r\ny").unwrap();
    std::fs::write(dir.join("empty.log"), b"").unwrap();
    std::fs::write(dir.join("full.log"), vec![b'z'; MAX_LOG_BYTES]).unwrap();
    let mut huge = vec![b'\n'; MAX_SCAN_BYTES as usize];
    huge.push(b'!');
    std::fs::write(dir.join("huge.log"), huge).unwrap();
    std::fs::create_dir(dir.join("nested")).unwrap();
    let list = listed(&data, &options());
    let files = &list.sessions[0].files;
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "a.log",
            "b.log",
            "empty.log",
            "full.log",
            "huge.log",
            "request.json"
        ]
    );
    let lines: Vec<Option<u64>> = files.iter().map(|f| f.lines).collect();
    assert_eq!(lines, [Some(2), Some(2), Some(0), Some(1), None, Some(1)]);
    let truncated: Vec<bool> = files.iter().map(|f| f.truncated).collect();
    assert_eq!(truncated, [false, false, false, true, true, false]);
    assert_eq!(files[3].bytes, MAX_LOG_BYTES as u64);
    assert!(!list.sessions[0].files_truncated);
    assert!(list.sessions[0].server_log.is_none());

    for n in 0..MAX_FILES {
        std::fs::write(dir.join(format!("extra-{n:03}.log")), b"").unwrap();
    }
    let list = listed(&data, &options());
    assert_eq!(list.sessions[0].files.len(), MAX_FILES);
    assert!(list.sessions[0].files_truncated);
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn the_scan_budget_bounds_one_call() {
    let data = scratch("sessions-budget");
    let dir = session(&data, &id('b'), t0());
    std::fs::write(dir.join("a.log"), b"1\n2\n3\n").unwrap();
    std::fs::write(dir.join("b.log"), b"1\n2\n3\n").unwrap();
    let mut options = options();
    options.scan_budget = 6;
    let list = listed(&data, &options);
    let lines: Vec<Option<u64>> = list.sessions[0].files.iter().map(|f| f.lines).collect();
    assert_eq!(lines, [Some(3), None, None]);
    assert_eq!(list.warnings.len(), 1);
    assert!(list.warnings[0].contains("Line counting stopped"));
    std::fs::remove_dir_all(data).unwrap();
}

const SERVER_LOG_TEXT: &str = concat!(
    "2026-10-18T08:19:41.000001Z  INFO nfs_server: listening\n",
    "2026-10-18T08:19:41Z  WARN nfs_server::diagnostics: unsupported operation received service=\"lsx\" connection=1 correlation=None route=Some(\"QueryFriends\") route_overflow=false seen=1 suppressed=0\n",
    "2026-10-18T08:19:42Z  WARN nfs_server::diagnostics: unsupported operation received service=\"lsx\" connection=1 correlation=None route=Some(\"QueryFriends\") route_overflow=false seen=2 suppressed=0\n",
    "2026-10-18T08:19:43Z  WARN nfs_server::net: unsupported operation received service=\"blaze\" connection=3 phase=Some(Login) route=Some(Route { component: 4, command: 22, category: 0, correlation: 51 }) disposition=\"closing\"\n",
    "2026-10-18T08:19:43Z  WARN nfs_server::net: unsupported operation received service=\"blaze\" connection=4 phase=Some(Login) route=Some(Route { component: 4, command: 22, category: 0, correlation: 52 }) disposition=\"closing\"\n",
    "2026-10-18T08:19:44Z  WARN nfs_server::auxiliary: unsupported operation received service=\"auxiliary\" connection=2 disposition=\"closing\"\n",
    "2026-10-18T08:19:45Z  WARN nfs_server::diagnostics: unsupported operation received service=\"lsx\" connection=1 correlation=Some(7) route=None route_overflow=true seen=1 suppressed=0\n",
    "2026-10-18T08:19:46Z ERROR nfs_server: handler failure\n",
    "2026-10-18T08:19:47Z DEBUG nfs_server: windows line\r\n",
    "2026-10-18T08:19:47Z TRACE nfs_server: detail\n",
    "thread 'main' panicked at src/main.rs:1:1:\n",
    "  continuation\n",
);

#[test]
fn the_server_log_is_counted_by_level_and_unsupported_route() {
    let data = scratch("sessions-log");
    let dir = session(&data, &id('5'), t0());
    let mut text = SERVER_LOG_TEXT.as_bytes().to_vec();
    text.extend(vec![b'x'; MAX_PARSED_LINE + 10]);
    text.push(b'\n');
    text.extend_from_slice(b"\xff\xfe binary\n");
    text.extend_from_slice(
        b"2026-10-18T08:19:48Z  WARN nfs_server: unsupported operation received service=\"cut",
    );
    std::fs::write(dir.join(SERVER_LOG), &text).unwrap();
    let list = listed(&data, &options());
    let log = list.sessions[0].server_log.clone().unwrap();
    assert_eq!(log.lines, 15);
    assert!(log.partial_last_line);
    assert_eq!(
        log.levels,
        Levels {
            error: 1,
            warn: 6,
            info: 1,
            debug: 1,
            trace: 1,
            other: 4,
        }
    );
    assert_eq!(log.unsupported_total, 6);
    let groups: Vec<(Option<&str>, Option<&str>, u64)> = log
        .unsupported
        .iter()
        .map(|g| (g.service.as_deref(), g.route.as_deref(), g.warnings))
        .collect();
    // Equal counts keep (service, route) order.
    assert_eq!(
        groups,
        [
            (
                Some("blaze"),
                Some("Route { component: 4, command: 22, category: 0 }"),
                2
            ),
            (Some("lsx"), Some("QueryFriends"), 2),
            (Some("auxiliary"), None, 1),
            (Some("lsx"), None, 1),
        ]
    );
    assert!(!log.unsupported_truncated);
    let file = list.sessions[0]
        .files
        .iter()
        .find(|f| f.name == SERVER_LOG)
        .unwrap();
    assert_eq!(file.lines, Some(15));
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn split_reads_parse_like_one_read_and_groups_are_bounded() {
    let mut whole = LogScanner::default();
    whole.push(SERVER_LOG_TEXT.as_bytes());
    let whole = whole.finish();
    let mut split = LogScanner::default();
    for chunk in SERVER_LOG_TEXT.as_bytes().chunks(7) {
        split.push(chunk);
    }
    assert_eq!(split.finish(), whole);
    assert!(!whole.partial_last_line);

    let mut many = LogScanner::default();
    let long = "r".repeat(MAX_FIELD_CHARS + 50);
    for n in 0..MAX_UNSUPPORTED_GROUPS + 5 {
        let route = if n == 0 {
            long.clone()
        } else {
            format!("Route{n}")
        };
        many.push(
            format!("2026-10-18T08:00:00Z  WARN s: unsupported operation received service=\"lsx\" route=Some(\"{route}\")\n")
                .as_bytes(),
        );
    }
    let many = many.finish();
    assert_eq!(many.unsupported.len(), MAX_UNSUPPORTED_GROUPS);
    assert!(many.unsupported_truncated);
    assert_eq!(many.unsupported_total, MAX_UNSUPPORTED_GROUPS as u64 + 5);
    let cut = many
        .unsupported
        .iter()
        .find_map(|g| g.route.clone().filter(|r| r.starts_with('r')))
        .unwrap();
    assert_eq!(cut.chars().count(), MAX_FIELD_CHARS + 1);
    assert!(cut.ends_with('\u{2026}'));
}

#[test]
fn a_route_correlation_is_dropped_wherever_it_appears() {
    for (route, expected) in [
        (
            "Route { component: 4, command: 22, category: 0, correlation: 51 }",
            "Route { component: 4, command: 22, category: 0 }",
        ),
        (
            "Route { correlation: 7, component: 4 }",
            "Route { component: 4 }",
        ),
        ("Route { correlation: 7 }", "Route {  }"),
        ("Route { correlation: none }", "Route { correlation: none }"),
        ("QueryFriends", "QueryFriends"),
    ] {
        assert_eq!(without_correlation(route.to_owned()), expected);
    }
}

#[test]
fn fields_are_read_from_quoted_and_debug_values() {
    let text = r#" a=1 service="a \"b\" c" phase=Some(Login) route=Some("x y") tail"#;
    assert_eq!(field(text, "a"), Some("1"));
    assert_eq!(
        field(text, "service").and_then(field_value).as_deref(),
        Some("a \"b\" c")
    );
    assert_eq!(
        field(text, "route").and_then(field_value).as_deref(),
        Some("x y")
    );
    assert_eq!(
        field(text, "phase").and_then(field_value).as_deref(),
        Some("Login")
    );
    assert_eq!(field(text, "missing"), None);
    assert_eq!(field("route=None", "route").and_then(field_value), None);
    assert_eq!(field("route=", "route").and_then(field_value), None);
    assert_eq!(
        field("route=\"unterminated", "route"),
        Some("\"unterminated")
    );
    assert_eq!(field("route=Some((", "route"), Some("Some(("));
    assert_eq!(
        field("=x route=\"\u{e5}\"", "route")
            .and_then(field_value)
            .as_deref(),
        Some("\u{e5}")
    );
}

#[test]
fn crash_dumps_inside_the_session_window_are_listed() {
    let base = scratch("sessions-dumps");
    let data = base.join("data");
    let dumps = base.join("Temp");
    std::fs::create_dir_all(&dumps).unwrap();
    let start = t0();
    let end = start + Duration::from_secs(600);
    let dir = session(&data, &id('d'), start);
    status(&dir, &id('d'), State::Failed, end);
    for (name, time) in [
        ("CrashDump_1.dmp", start - Duration::from_secs(10)),
        ("CrashDump_2.dmp", start + Duration::from_secs(5)),
        ("CrashDump_3.DMP", end + Duration::from_secs(60)),
        ("CrashDump_4.dmp", end + Duration::from_secs(600)),
        ("other.dmp", start + Duration::from_secs(5)),
        ("CrashDump_5.txt", start + Duration::from_secs(5)),
    ] {
        std::fs::write(dumps.join(name), b"MDMP").unwrap();
        set_time(&dumps.join(name), time);
    }
    std::fs::create_dir(dumps.join("CrashDump_6.dmp")).unwrap();
    let mut options = options();
    options.crash_dump_directory = Some(dumps.clone());
    options.ea_desktop_logs = Some(base.join("EA Desktop").join("Logs"));
    let list = listed(&data, &options);
    let names: Vec<String> = list.sessions[0]
        .crash_dumps
        .as_ref()
        .unwrap()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    assert_eq!(names, ["CrashDump_2.dmp", "CrashDump_3.DMP"]);
    let dump = &list.sessions[0].crash_dumps.as_ref().unwrap()[0];
    assert_eq!(dump.bytes, 4);
    assert_eq!(dump.modified_utc, "2026-10-18T08:00:05Z");
    assert_eq!(dump.path, dumps.join("CrashDump_2.dmp"));
    assert!(list.crash_dump_directory.as_ref().unwrap().exists);
    assert!(!list.ea_desktop_logs.as_ref().unwrap().exists);

    // A running session's window ends now.
    let live = session(&data, &id('e'), end);
    status(&live, &id('e'), State::Ready, end);
    session::point_current_session(&data, &id('e')).unwrap();
    let held = session::Lock::acquire(&data).unwrap();
    std::fs::write(dumps.join("CrashDump_7.dmp"), b"MDMP").unwrap();
    set_time(
        &dumps.join("CrashDump_7.dmp"),
        options.now - Duration::from_secs(1),
    );
    let list = listed(&data, &options);
    let live = list.sessions.iter().find(|s| s.id == id('e')).unwrap();
    assert_eq!(live.state, Some(State::Ready));
    let names: Vec<&str> = live
        .crash_dumps
        .as_ref()
        .unwrap()
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["CrashDump_3.DMP", "CrashDump_4.dmp", "CrashDump_7.dmp"]
    );
    drop(held);

    options.crash_dump_directory = Some(base.join("absent"));
    let list = listed(&data, &options);
    assert!(!list.crash_dump_directory.as_ref().unwrap().exists);
    assert!(
        list.sessions
            .iter()
            .all(|s| s.crash_dumps == Some(Vec::new()))
    );
    options.crash_dump_directory = None;
    let list = listed(&data, &options);
    assert!(list.sessions.iter().all(|s| s.crash_dumps.is_none()));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn the_recording_folder_is_reported_when_present() {
    let data = scratch("sessions-runs");
    session(&data, &id('7'), t0());
    let list = listed(&data, &options());
    assert!(!list.sessions[0].runs_directory_exists);
    assert_eq!(
        list.sessions[0].runs_directory,
        data.join("artifacts").join("runs").join(id('7'))
    );
    std::fs::create_dir_all(&list.sessions[0].runs_directory).unwrap();
    assert!(listed(&data, &options()).sessions[0].runs_directory_exists);
    std::fs::remove_dir_all(data).unwrap();
}

#[cfg(unix)]
#[test]
fn links_are_never_followed_into_or_inside_a_session() {
    let base = scratch("sessions-links");
    let data = base.join("data");
    let outside = base.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.log"), b"x\n").unwrap();
    let dir = session(&data, &id('8'), t0());
    std::os::unix::fs::symlink(outside.join("secret.log"), dir.join("linked.log")).unwrap();
    std::os::unix::fs::symlink(&outside, data.join("artifacts/launcher").join(id('9'))).unwrap();
    let list = listed(&data, &options());
    assert_eq!(ids(&list), [id('8')]);
    assert!(
        list.sessions[0]
            .files
            .iter()
            .all(|f| f.name != "linked.log")
    );
    std::fs::remove_dir_all(base).unwrap();
}
