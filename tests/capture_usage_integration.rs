//! Synthetic core qualification only. Native command integration remains disabled.
use keito_cli::capture::{self, Binding, Clock, Repository, Signal, Store, UsageSource};
use keito_cli::cli::capture::CaptureEvent::{self, *};
use serde_json::{json, Value};
use std::fs;
const SESSION: &str = "12345678-1234-4234-8234-123456789abc";
fn clock(n: i64) -> Clock { Clock { wall: 1_794_067_200+n, monotonic: 1000+n, boot: "fixture".into() } }
fn repo() -> Repository { Repository { key: capture::hash("repo"), remote_hash: capture::hash("remote"), hint_hash: capture::hash("hint"), linked_without_hint: false, disabled: false } }
struct Fixture { dir: tempfile::TempDir, home: std::path::PathBuf, path: std::path::PathBuf, store: Store, binding: Binding }
impl Fixture {
 fn new() -> Self {
  let dir=tempfile::tempdir().unwrap(); let home=dir.path().canonicalize().unwrap().join("home");
  let path=home.join(format!(".claude/projects/fixture/{SESSION}.jsonl")); fs::create_dir_all(path.parent().unwrap()).unwrap();
  let mut store=Store::open(&dir.path().join("capture")).unwrap();
  let binding=store.approve(Binding {repository_key:repo().key,remote_hash:repo().remote_hash,hint_hash:repo().hint_hash,id:String::new(),version:0,workspace_id:"co_fixture".into(),owner_id:"user_fixture".into(),origin_hash:capture::hash("origin"),project_id:"project_fixture".into(),task_id:"task_fixture".into(),agents:vec!["claude-code".into()],paused:false,revoked:false}).unwrap();
  Self {dir,home,path,store,binding}
 }
 fn write(&self, records: Vec<Value>) { fs::write(&self.path,records.into_iter().map(|v|v.to_string()+"\n").collect::<String>()).unwrap(); }
 fn event(&mut self,event:CaptureEvent,n:i64,usage:bool) {
  let source=UsageSource::qualified_fixture(&self.home,&self.path,"claude-code",SESSION);
  self.store.event_with_usage(&repo(),Signal {source:"claude-code",resumed:false,source_id:SESSION,turn_id:Some("prompt_1"),version:"2.1.319",event,clock:&clock(n)},if usage {Some(&source)} else {None}).unwrap();
 }
 fn bodies(&self)->Vec<Value> { let db=rusqlite::Connection::open(self.dir.path().join("capture/capture.sqlite3")).unwrap(); let mut s=db.prepare("SELECT body FROM outbox ORDER BY revision").unwrap(); s.query_map([],|r|r.get::<_,String>(0)).unwrap().map(|s|serde_json::from_str(&s.unwrap()).unwrap()).collect() }
}
fn request(id:&str, output:u64)->Value { json!({"type":"assistant","sessionId":SESSION,"message":{"id":id,"model":"claude-opus-5-5","usage":{"input_tokens":2,"cache_read_input_tokens":null,"cache_creation_input_tokens":2280,"output_tokens":output},"content":[{"text":"PRIVATE_CANARY sk-secret /private/customer"}]}}) }
#[test]
fn exact_session_usage_and_time_commit_atomically_without_prose_or_paths() {
 let mut f=Fixture::new(); f.write(vec![request("msg_1",5)]); f.event(Prompt,0,false); f.event(Stop,59,true);
 let b=&f.bodies()[0]; assert_eq!(b["segments"][0]["duration_seconds"],59); assert_eq!(b["usage"]["status"],"PARTIAL");
 assert_eq!(b["usage"]["records"][0]["cached_input_tokens"],Value::Null); assert_eq!(b["usage"]["records"][0]["cache_write_input_tokens"],2280);
 let db=fs::read(f.dir.path().join("capture/capture.sqlite3")).unwrap(); let text=String::from_utf8_lossy(&db); assert!(!text.contains("PRIVATE_CANARY")); assert!(!text.contains("sk-secret")); assert!(!text.contains(f.path.to_str().unwrap()));
 assert!(f.store.claim(&f.binding,clock(60).wall).unwrap().is_some());
}
#[test]
fn late_usage_has_its_own_revision_and_replay_changes_no_frozen_bytes() {
 let mut f=Fixture::new(); f.event(Prompt,0,false); f.event(Stop,1,false); let first=f.bodies();
 f.write(vec![request("msg_1",5)]); f.event(Stop,10,true); f.event(Stop,11,true);
 let b=f.bodies(); assert_eq!(b.len(),2); assert_eq!(b[0],first[0]); assert_eq!(b[1]["segments"],json!([])); assert_eq!(b[1]["revision"],2);
 assert_eq!(f.store.preview(SESSION).unwrap()[0].segments.len(),1);
}
#[test]
fn more_than_25_requests_are_disjoint_bounded_batches_without_truncation() {
 let mut f=Fixture::new(); f.write((0..80).map(|n|request(&format!("msg_{n:03}_{}","a".repeat(110)),5)).collect()); f.event(Prompt,0,false); f.event(Stop,2,true);
 let b=f.bodies(); assert!(b.len()>3); let mut ids=std::collections::BTreeSet::new(); let mut seconds=0;
 for body in b { assert!(body.to_string().len()<=capture::MAX_BODY_BYTES); let r=body["usage"]["records"].as_array().unwrap(); assert!(r.len()<=25); for row in r {assert!(ids.insert(row["source_record_id"].as_str().unwrap().to_owned()));} seconds+=body["segments"].as_array().unwrap().iter().map(|s|s["duration_seconds"].as_i64().unwrap()).sum::<i64>(); }
 assert_eq!(ids.len(),80); assert_eq!(seconds,2);
}
#[test]
fn conflicting_request_identity_holds_unsent_evidence_but_preserves_time() {
 let mut f=Fixture::new(); f.write(vec![request("msg_1",5)]); f.event(Prompt,0,false); f.event(Stop,2,true); let frozen=f.bodies();
 f.write(vec![request("msg_1",6)]); f.event(Stop,3,true);
 assert!(f.store.claim(&f.binding,clock(5).wall).unwrap().is_none()); assert!(f.store.preview(SESSION).unwrap()[0].needs_attention); assert_eq!(f.bodies(),frozen); assert_eq!(f.store.preview(SESSION).unwrap()[0].segments[0].duration_seconds,2);
 assert!(f.store.status().unwrap().queue.iter().any(|q|q.reason.as_deref()==Some("USAGE_RECORD_CONFLICT")));
}
#[test]
fn conflict_within_one_snapshot_is_held_instead_of_omitting_conflicted_id() {
 let mut f=Fixture::new(); f.write(vec![request("msg_1",1),request("msg_1",5),request("msg_2",3)]); f.event(Prompt,0,false); f.event(Stop,2,true);
 assert!(f.store.claim(&f.binding,clock(3).wall).unwrap().is_none()); assert!(f.store.preview(SESSION).unwrap()[0].needs_attention); assert_eq!(f.store.preview(SESSION).unwrap()[0].segments[0].duration_seconds,2);
}
#[test]
fn revoked_binding_never_reads_or_accepts_transcript_usage() {
 let mut f=Fixture::new(); f.event(Prompt,0,false); f.store.control(&repo(),"disable",&clock(1)).unwrap(); f.write(vec![request("msg_1",5)]); f.event(Stop,2,true);
 assert!(f.bodies().iter().all(|b|b["usage"]["status"]=="UNAVAILABLE"));
}
#[test]
fn missing_unsafe_and_wrong_identity_sources_never_replace_prior_usage() {
 let mut f=Fixture::new(); f.write(vec![request("msg_1",5)]); f.event(Prompt,0,false); f.event(Stop,2,true);
 let prior=f.bodies(); fs::write(&f.path,"{\"type\":\"assistant\",\"sessionId\":\"other\"}\n").unwrap(); f.event(Stop,3,true); assert_eq!(f.bodies(),prior);
 let s=UsageSource::qualified_fixture(&f.home,&f.path,"claude-code","different-session");
 f.store.event_with_usage(&repo(),Signal {source:"claude-code",resumed:false,source_id:SESSION,turn_id:Some("prompt_1"),version:"2.1.319",event:Stop,clock:&clock(4)},Some(&s)).unwrap(); assert_eq!(f.bodies(),prior);
}
#[test]
fn legacy_metadata_capture_never_reads_transcripts_without_explicit_internal_gate() {
 let mut f=Fixture::new(); f.write(vec![request("msg_1",5)]); f.event(Prompt,0,false); f.event(Stop,2,false); assert_eq!(f.bodies()[0]["usage"]["status"],"UNAVAILABLE");
}
