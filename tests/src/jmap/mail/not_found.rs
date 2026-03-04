/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use serde_json::json;

use crate::jmap::JMAPTest;

/// Regression test for JMAP RFC 8620 §5.1 notFound compliance.
///
/// The JMAP spec requires that any requested ID that is not found in the
/// store MUST appear in the `notFound` array of the response. Stalwart
/// uses a custom base32 encoding for IDs, and IDs containing characters
/// outside this alphabet (e.g., dashes, underscores, certain digits) were
/// silently dropped during request parsing instead of being reported as
/// notFound.
///
/// This test verifies that:
/// - IDs with invalid base32 characters appear in notFound
/// - Valid-looking but nonexistent IDs appear in notFound
/// - A mix of valid existing, valid nonexistent, and unparseable IDs
///   produces the correct notFound array
pub async fn test(params: &mut JMAPTest) {
    println!("Running notFound compliance tests...");
    let account = params.account("jdoe@example.com");

    // -------------------------------------------------------------------------
    // Test 1: Email/get with IDs containing invalid base32 characters
    //
    // Stalwart's base32 alphabet is: abcdefghijklmnopqrstuvwxyz792013
    // Characters like '-', '_', '4', '5', '6', '8' are NOT in the alphabet
    // and cause Id::from_str() to fail. Previously these were silently
    // dropped; now they must appear in notFound.
    // -------------------------------------------------------------------------
    let invalid_ids = [
        "nonexistent-email-xyz",   // dashes are invalid
        "bad_id_with_underscores", // underscores are invalid
        "contains456digits",       // digits 4,5,6 are invalid
    ];

    let response = account
        .jmap_method_call(
            "Email/get",
            json!({
                "accountId": account.id_string(),
                "ids": invalid_ids,
                "properties": ["id"]
            }),
        )
        .await;

    let not_found: Vec<&str> = response.not_found().collect();
    for invalid_id in &invalid_ids {
        assert!(
            not_found.contains(invalid_id),
            "Email/get: ID {:?} should be in notFound but wasn't. Got: {:?}",
            invalid_id,
            not_found,
        );
    }
    assert!(
        response.list().is_empty(),
        "Email/get: list should be empty when all IDs are invalid"
    );

    // -------------------------------------------------------------------------
    // Test 2: Mailbox/get with invalid IDs
    // -------------------------------------------------------------------------
    let response = account
        .jmap_method_call(
            "Mailbox/get",
            json!({
                "accountId": account.id_string(),
                "ids": ["fake-mailbox-id", "another_bad_one"],
                "properties": ["id", "name"]
            }),
        )
        .await;

    let not_found: Vec<&str> = response.not_found().collect();
    assert!(
        not_found.contains(&"fake-mailbox-id"),
        "Mailbox/get: 'fake-mailbox-id' should be in notFound. Got: {:?}",
        not_found,
    );
    assert!(
        not_found.contains(&"another_bad_one"),
        "Mailbox/get: 'another_bad_one' should be in notFound. Got: {:?}",
        not_found,
    );

    // -------------------------------------------------------------------------
    // Test 3: Thread/get with invalid IDs
    // -------------------------------------------------------------------------
    let response = account
        .jmap_method_call(
            "Thread/get",
            json!({
                "accountId": account.id_string(),
                "ids": ["thread-does-not-exist"],
            }),
        )
        .await;

    let not_found: Vec<&str> = response.not_found().collect();
    assert!(
        not_found.contains(&"thread-does-not-exist"),
        "Thread/get: 'thread-does-not-exist' should be in notFound. Got: {:?}",
        not_found,
    );

    // -------------------------------------------------------------------------
    // Test 4: Mix of valid-format (but nonexistent) and invalid-format IDs
    //
    // "ba" is a valid base32 string (Id=32) but won't match any existing email.
    // Note: the server normalizes IDs, so "ba" round-trips as "ba".
    // "not-valid" contains a dash so it fails base32 parsing.
    // Both should end up in notFound.
    // -------------------------------------------------------------------------
    let response = account
        .jmap_method_call(
            "Email/get",
            json!({
                "accountId": account.id_string(),
                "ids": ["ba", "not-valid"],
                "properties": ["id"]
            }),
        )
        .await;

    let not_found: Vec<&str> = response.not_found().collect();
    assert_eq!(
        not_found.len(),
        2,
        "Email/get: both valid-nonexistent and invalid IDs should be in notFound. Got: {:?}",
        not_found,
    );
    assert!(
        not_found.contains(&"not-valid"),
        "Email/get: invalid-format 'not-valid' should be in notFound. Got: {:?}",
        not_found,
    );

    // -------------------------------------------------------------------------
    // Test 5: Identity/get with invalid IDs
    // -------------------------------------------------------------------------
    let response = account
        .jmap_method_call(
            "Identity/get",
            json!({
                "accountId": account.id_string(),
                "ids": ["identity-xyz-456"],
                "properties": ["id", "name"]
            }),
        )
        .await;

    let not_found: Vec<&str> = response.not_found().collect();
    assert!(
        not_found.contains(&"identity-xyz-456"),
        "Identity/get: 'identity-xyz-456' should be in notFound. Got: {:?}",
        not_found,
    );

    println!("notFound compliance tests passed.");
}
