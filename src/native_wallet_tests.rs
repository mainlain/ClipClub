// Included inside crypto.rs's test module to exercise the real local signing flow.

const NATIVE_PASSWORD: &str = "wallet integration test password";
const NATIVE_ADDRESS: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";

async fn native_mock() -> Mock {
    let m = mock().await;
    let result = m
        .crypto
        .create_local_wallet(
            zeroize::Zeroizing::new(NATIVE_PASSWORD.to_owned()),
            Some(zeroize::Zeroizing::new(format!("{}1", "0".repeat(63)))),
            None,
        )
        .await
        .unwrap();
    assert_eq!(result["address"], NATIVE_ADDRESS);
    assert_eq!(m.crypto.wallet().await.unwrap(), NATIVE_ADDRESS);
    m
}

async fn native_review(m: &Mock) -> String {
    let request: TransactionRequest =
        serde_json::from_value(json!({"action":"register_seeder"})).unwrap();
    let transaction = m.crypto.transaction(request, PEER).await.unwrap();
    assert_eq!(transaction["from"], NATIVE_ADDRESS);
    assert_eq!(transaction["to"], CONTRACT);
    assert_eq!(transaction["value"], "0x0");
    assert_eq!(transaction["wallet_approval"]["gas_limit"], "120000");
    assert_eq!(
        transaction["wallet_approval"]["gas_price_wei"],
        "1000000000"
    );
    assert_eq!(
        transaction["wallet_approval"]["max_fee_wei"],
        "120000000000000"
    );
    transaction["wallet_approval"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn native_sends(m: &Mock) -> Vec<Value> {
    m.state
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request["method"] == "eth_sendRawTransaction")
        .cloned()
        .collect()
}

#[tokio::test]
async fn native_send_requires_the_explicit_current_review_id() {
    let m = native_mock().await;
    assert!(m.crypto.send_local_transaction("").await.is_err());
    let id = native_review(&m).await;
    assert_eq!(id.len(), 64);
    assert!(
        m.crypto
            .send_local_transaction(&"f".repeat(64))
            .await
            .is_err()
    );
    assert!(native_sends(&m).is_empty());
    assert!(m.crypto.journal.lock().await.pending_transaction.is_none());
    let sent = m.crypto.send_local_transaction(&id).await.unwrap();
    assert!(sent.get("broadcast_error").is_none());
    let sends = native_sends(&m);
    assert_eq!(sends.len(), 1);
    let journal = load_journal(&m.crypto.path).unwrap();
    let saved = journal.local_submission.as_ref().unwrap();
    assert_eq!(sent["hash"], saved.hash);
    assert_eq!(sends[0]["params"][0], saved.raw);
    crate::wallet::validate_signed_transaction(
        &saved.raw,
        &saved.hash,
        m.crypto.chain_id,
        NATIVE_ADDRESS,
        CONTRACT,
    )
    .unwrap();
    assert!(m.crypto.send_local_transaction(&id).await.is_err());
    assert_eq!(native_sends(&m).len(), 1);
}

#[tokio::test]
async fn native_concurrent_approvals_broadcast_only_once() {
    let m = native_mock().await;
    let id = native_review(&m).await;
    let (first, second) = tokio::join!(
        m.crypto.send_local_transaction(&id),
        m.crypto.send_local_transaction(&id)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(native_sends(&m).len(), 1);
    assert!(
        load_journal(&m.crypto.path)
            .unwrap()
            .local_submission
            .is_some()
    );
}

#[tokio::test]
async fn native_ambiguous_broadcast_keeps_signed_bytes_and_recovers_while_locked() {
    let m = native_mock().await;
    let id = native_review(&m).await;
    m.state.fixture.lock().unwrap().send_error = true;
    let response = m.crypto.send_local_transaction(&id).await.unwrap();
    assert!(
        response["broadcast_error"]
            .as_str()
            .unwrap()
            .contains("Broadcast")
    );
    let disk = load_journal(&m.crypto.path).unwrap();
    let saved = disk.local_submission.as_ref().unwrap().clone();
    assert_eq!(response["hash"], saved.hash);
    assert_eq!(disk.pending_transaction.as_ref().unwrap().hash, saved.hash);
    assert_eq!(native_sends(&m)[0]["params"][0], saved.raw);

    // Rehydrate both durable state and the vault, with no decrypted key or approval.
    m.crypto.lock_local_wallet().await.unwrap();
    *m.crypto.journal.lock().await = load_journal(&m.crypto.path).unwrap();
    *m.crypto.vault.lock().unwrap() =
        crate::wallet::WalletVault::open(m._directory.path()).unwrap();
    assert_eq!(m.crypto.local_status().await["unlocked"], false);
    assert!(m.crypto.approval.lock().await.is_none());
    {
        let mut fixture = m.state.fixture.lock().unwrap();
        fixture.send_error = false;
        fixture.receipt = Value::Null;
    }
    let retried = m.crypto.rebroadcast_local_transaction().await.unwrap();
    assert_eq!(retried["hash"], saved.hash);
    assert!(retried.get("broadcast_error").is_none());
    let sends = native_sends(&m);
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[0]["params"], sends[1]["params"]);
    assert_eq!(m.crypto.local_status().await["unlocked"], false);
    assert_eq!(
        load_journal(&m.crypto.path)
            .unwrap()
            .local_submission
            .unwrap()
            .raw,
        saved.raw
    );
}

#[tokio::test]
async fn native_confirmed_submission_is_not_broadcast_again() {
    let m = native_mock().await;
    let id = native_review(&m).await;
    let response = m.crypto.send_local_transaction(&id).await.unwrap();
    m.state.fixture.lock().unwrap().receipt =
        json!({"transactionHash":response["hash"],"status":"0x1"});
    m.crypto.lock_local_wallet().await.unwrap();
    let recovered = m.crypto.rebroadcast_local_transaction().await.unwrap();
    assert_eq!(recovered["hash"], response["hash"]);
    assert_eq!(native_sends(&m).len(), 1);
}

#[tokio::test]
async fn native_journal_corruption_is_rejected_before_rebroadcast() {
    let m = native_mock().await;
    let id = native_review(&m).await;
    m.crypto.send_local_transaction(&id).await.unwrap();
    let original = std::fs::read(&m.crypto.path).unwrap();
    let mut corrupt: Value = serde_json::from_slice(&original).unwrap();
    corrupt["local_submission"]["raw"] = json!("0xc0");
    std::fs::write(&m.crypto.path, serde_json::to_vec(&corrupt).unwrap()).unwrap();
    assert!(load_journal(&m.crypto.path).is_err());
    std::fs::write(&m.crypto.path, &original).unwrap();

    // A journal hash cannot be replaced independently of the pending record.
    let mut corrupt: Value = serde_json::from_slice(&original).unwrap();
    corrupt["local_submission"]["hash"] = json!(HASH);
    std::fs::write(&m.crypto.path, serde_json::to_vec(&corrupt).unwrap()).unwrap();
    assert!(load_journal(&m.crypto.path).is_err());
    std::fs::write(&m.crypto.path, original).unwrap();

    // Recovery also revalidates the bytes in memory before touching the network.
    m.crypto
        .journal
        .lock()
        .await
        .local_submission
        .as_mut()
        .unwrap()
        .raw = "0xc0".into();
    assert!(m.crypto.rebroadcast_local_transaction().await.is_err());
    assert_eq!(native_sends(&m).len(), 1);
}

#[tokio::test]
async fn native_changed_nonce_and_expired_review_require_a_new_review() {
    let m = native_mock().await;
    let id = native_review(&m).await;
    m.state.fixture.lock().unwrap().nonce = "0x1".into();
    let error = m.crypto.send_local_transaction(&id).await.unwrap_err();
    assert!(error.to_string().contains("nonce changed"));
    assert!(native_sends(&m).is_empty());
    assert!(
        load_journal(&m.crypto.path)
            .unwrap()
            .pending_transaction
            .is_none()
    );

    let id = native_review(&m).await;
    m.crypto.approval.lock().await.as_mut().unwrap().created =
        Instant::now() - Duration::from_secs(121);
    assert!(m.crypto.send_local_transaction(&id).await.is_err());
    assert!(native_sends(&m).is_empty());
}

#[tokio::test]
async fn native_lock_revokes_review_and_unlock_requires_fresh_approval() {
    let m = native_mock().await;
    let id = native_review(&m).await;
    m.crypto.lock_local_wallet().await.unwrap();
    assert!(m.crypto.approval.lock().await.is_none());
    assert!(m.crypto.send_local_transaction(&id).await.is_err());
    let request: TransactionRequest =
        serde_json::from_value(json!({"action":"register_seeder"})).unwrap();
    assert!(m.crypto.transaction(request, PEER).await.is_err());
    m.crypto
        .unlock_local_wallet(zeroize::Zeroizing::new(NATIVE_PASSWORD.into()))
        .await
        .unwrap();
    assert!(m.crypto.send_local_transaction(&id).await.is_err());
    assert_ne!(native_review(&m).await, id);
    assert!(native_sends(&m).is_empty());
}

#[tokio::test]
async fn native_session_cancellation_revokes_checkout_and_other_action_reviews() {
    let m = native_mock().await;
    let draft = draft_two(&m).await;
    let checkout = m
        .crypto
        .transaction(checkout_request(&draft), PEER)
        .await
        .unwrap();
    let id = checkout["wallet_approval"]["id"].as_str().unwrap();
    m.crypto.cancel_rating_preparation().await.unwrap();
    assert_eq!(m.crypto.session_snapshot().await.unwrap().status, "draft");
    assert!(m.crypto.send_local_transaction(id).await.is_err());

    let other_action = native_review(&m).await;
    m.crypto.cancel_rating_session().await.unwrap();
    assert!(m.crypto.session_snapshot().await.is_none());
    assert!(
        m.crypto
            .send_local_transaction(&other_action)
            .await
            .is_err()
    );
    assert!(native_sends(&m).is_empty());
}

#[tokio::test]
async fn native_failed_durable_save_never_broadcasts_or_marks_pending() {
    let m = native_mock().await;
    let id = native_review(&m).await;
    std::fs::create_dir(m.crypto.path.with_extension("json.tmp")).unwrap();
    assert!(m.crypto.send_local_transaction(&id).await.is_err());
    assert!(native_sends(&m).is_empty());
    let disk = load_journal(&m.crypto.path).unwrap();
    assert!(disk.pending_transaction.is_none());
    assert!(disk.local_submission.is_none());
    let memory = m.crypto.journal.lock().await;
    assert!(memory.pending_transaction.is_none());
    assert!(memory.local_submission.is_none());
}

#[tokio::test]
async fn native_verified_replacement_discards_the_superseded_signed_bytes() {
    let m = native_mock().await;
    let draft = draft_two(&m).await;
    let checkout = m.crypto.transaction(checkout_request(&draft), PEER).await.unwrap();
    m.crypto.send_local_transaction(checkout["wallet_approval"]["id"].as_str().unwrap()).await.unwrap();
    assert!(m.crypto.journal.lock().await.local_submission.is_some());
    let receipt = serde_json::to_string(&session_receipt(&draft)).unwrap().replace(&WALLET[2..], &NATIVE_ADDRESS[2..]);
    m.state.fixture.lock().unwrap().receipt = serde_json::from_str(&receipt).unwrap();
    m.crypto.session_payment(HASH, PEER).await.unwrap();
    let journal = load_journal(&m.crypto.path).unwrap();
    assert_eq!(journal.pending_transaction.unwrap().hash, HASH);
    assert!(journal.local_submission.is_none());
    assert_eq!(m.crypto.rebroadcast_local_transaction().await.unwrap()["hash"], HASH);
    assert_eq!(native_sends(&m).len(), 1);
}
