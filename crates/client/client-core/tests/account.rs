//! The account store on the client core (v12): asked for right after the login, kept in `account()`, changes sent
//! with `save_account` once logged in.

mod common;

use client_core::ClientEvent;
use common::*;
use protocol::{AccountSave, ClientMsg};

fn save(set: &[(&str, &str)], remove: &[&str]) -> AccountSave {
    AccountSave {
        set: set
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
        remove: remove.iter().map(|k| (*k).to_string()).collect(),
    }
}

#[test]
fn the_store_is_asked_for_after_the_login_and_kept() {
    let mut n = Net::start();
    n.srv.account = vec![("GridOn".into(), "0".into())];
    n.connect();
    assert!(n.core.account().is_none(), "nothing before the login");
    n.login();
    assert_eq!(
        n.srv
            .sent(|m| matches!(m, ClientMsg::Account(s) if s.is_empty())),
        1,
        "one request"
    );
    assert!(n.has_event(|e| matches!(e, ClientEvent::AccountLoaded)));
    let a = n.core.account().expect("the store");
    assert_eq!(a.prefs, [("GridOn".to_string(), "0".to_string())]);
}

#[test]
fn a_change_goes_to_the_server_and_the_local_copy_follows() {
    let mut n = Net::start();
    n.connect();
    // Not logged in: refused, nothing sent.
    assert!(!n.core.save_account(save(&[("A", "1")], &[])));
    n.login();
    assert!(
        n.core
            .save_account(save(&[("YieldOn", "1"), ("GridOn", "0")], &[]))
    );
    n.settle();
    assert_eq!(n.srv.account.len(), 2);
    assert!(n.core.save_account(save(&[], &["GridOn"])));
    n.settle();
    assert_eq!(n.srv.account, [("YieldOn".to_string(), "1".to_string())]);
    assert_eq!(
        n.core.account().unwrap().prefs,
        [("YieldOn".to_string(), "1".to_string())]
    );
    // An empty or invalid change is not sent.
    let sent = n.srv.sent(|m| matches!(m, ClientMsg::Account(_)));
    assert!(!n.core.save_account(AccountSave::default()));
    assert!(!n.core.save_account(save(&[("bad key", "1")], &[])));
    n.settle();
    assert_eq!(n.srv.sent(|m| matches!(m, ClientMsg::Account(_))), sent);
}
