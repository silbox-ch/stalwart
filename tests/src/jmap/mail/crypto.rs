/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::{JMAPTest, ManagementApi, mail::delivery::SmtpConnection};
use email::message::crypto::{
    Algorithm, EncryptMessage, EncryptionMethod, EncryptionParams, EncryptionType, try_parse_certs,
};
use mail_parser::{MessageParser, MimeHeaders};
use std::path::PathBuf;
use store::{
    Deserialize, Serialize,
    write::{Archive, Archiver},
};

pub async fn test(params: &mut JMAPTest) {
    println!("Running Encryption-at-rest tests...");

    // Check encryption
    check_is_encrypted();
    import_certs_and_encrypt().await;

    // Create test account
    let account = params.account("jdoe@example.com");
    let client = account.client();

    // Build API
    let api = ManagementApi::new(8899, "jdoe@example.com", "12345");

    // Try importing using multiple methods and symmetric algos
    for (file_name, method, num_certs) in [
        ("cert_smime.pem", EncryptionMethod::SMIME, 3),
        ("cert_pgp.pem", EncryptionMethod::PGP, 1),
    ] {
        let certs = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("crypto")
                .join(file_name),
        )
        .unwrap();

        for algo in [Algorithm::Aes128, Algorithm::Aes256] {
            let request = match method {
                EncryptionMethod::PGP => EncryptionType::PGP {
                    algo,
                    certs: certs.clone(),
                    allow_spam_training: true,
                },
                EncryptionMethod::SMIME => EncryptionType::SMIME {
                    algo,
                    certs: certs.clone(),
                    allow_spam_training: true,
                },
            };

            assert_eq!(
                api.post::<u32>("/api/account/crypto", &request)
                    .await
                    .unwrap()
                    .unwrap_data(),
                num_certs
            );
        }
    }

    // Send a new message, which should be encrypted
    let mut lmtp = SmtpConnection::connect().await;
    lmtp.ingest(
        "bill@example.com",
        &["jdoe@example.com"],
        concat!(
            "From: bill@example.com\r\n",
            "To: jdoe@example.com\r\n",
            "Subject: TPS Report (should be encrypted)\r\n",
            "\r\n",
            "I'm going to need those TPS reports ASAP. ",
            "So, if you could do that, that'd be great."
        ),
    )
    .await;

    // Send an encrypted message
    lmtp.ingest(
        "bill@example.com",
        &["jdoe@example.com"],
        concat!(
            "From: bill@example.com\r\n",
            "To: jdoe@example.com\r\n",
            "Subject: TPS Report (already encrypted)\r\n",
            "Content-Type: application/pkcs7-mime; name=\"smime.p7m\"; smime-type=enveloped-data\r\n",
            "\r\n",
            "xjMEZMYfNhYJKwYBBAHaRw8BAQdAYyTN1HzqapLw8xwkCGwa0OjsgT/JqhcB/+Dy",
            "Ga1fsBrNG0pvaG4gRG9lIDxqb2huQGV4YW1wbGUub3JnPsKJBBMWCAAxFiEEg836",
            "pwbXpuQ/THMtpJwd4oBfIrUFAmTGHzYCGwMECwkIBwUVCAkKCwUWAgMBAAAKCRCk",
            "nB3igF8itYhyAQD2jEdeYa3gyQ47X9YWZTK1wEJkN8W9//V1fYl2XQwqlQEA0qBv",
            "Ai6nUh99oDw+/zQ8DFIKdeb5Ti4tu/X58PdpiQ7OOARkxh82EgorBgEEAZdVAQUB",
            "AQdAvXz2FbFN0DovQF/ACnZyczTsSIQp0mvmF1PE+aijbC8DAQgHwngEGBYIACAW",
            "IQSDzfqnBtem5D9Mcy2knB3igF8itQUCZMYfNgIbDAAKCRCknB3igF8itRnoAQC3",
            "GzPmgx7TnB+SexPuJV/DoKSMJ0/X+hbEFcZkulxaDQEAh+xiJCvf+ZNAKw6kFhsL",
            "UuZhEDktxnY6Ehz3aB7FawA=",
            "=KGrr",
        ),
    )
    .await;

    // Disable encryption
    assert_eq!(
        api.post::<Option<String>>("/api/account/crypto", &EncryptionType::Disabled)
            .await
            .unwrap()
            .unwrap_data(),
        None
    );

    // Send a new message, which should NOT be encrypted
    lmtp.ingest(
        "bill@example.com",
        &["jdoe@example.com"],
        concat!(
            "From: bill@example.com\r\n",
            "To: jdoe@example.com\r\n",
            "Subject: TPS Report (plain text)\r\n",
            "\r\n",
            "I'm going to need those TPS reports ASAP. ",
            "So, if you could do that, that'd be great."
        ),
    )
    .await;

    // Check messages
    let mut request = client.build();
    request.get_email();
    let emails = request.send_get_email().await.unwrap().take_list();
    assert_eq!(emails.len(), 3, "3 messages were expected: {:#?}.", emails);

    for email in emails {
        let message =
            String::from_utf8(client.download(email.blob_id().unwrap()).await.unwrap()).unwrap();
        if message.contains("should be encrypted") {
            assert!(
                message.contains("Content-Type: multipart/encrypted"),
                "got message {message}, expected encrypted message"
            );
        } else if message.contains("already encrypted") {
            assert!(
                message.contains("Content-Type: application/pkcs7-mime")
                    && message.contains("xjMEZMYfNhYJKwYBBAHaRw8BAQdAYy"),
                "got message {message}, expected message to be left intact"
            );
        } else if message.contains("plain text") {
            assert!(
                message.contains("I'm going to need those TPS reports ASAP."),
                "got message {message}, expected plain text message"
            );
        } else {
            panic!("Unexpected message: {:#?}", message)
        }
    }
}

pub async fn test_admin_crypto_impersonation(params: &mut JMAPTest) {
    println!("Running admin crypto impersonation tests...");

    // Admin API (has Impersonate + ManageEncryption permissions)
    let admin_api = ManagementApi::new(8899, "admin", "secret");

    // Regular user API (has ManageEncryption but NOT Impersonate)
    let user_api = ManagementApi::new(8899, "jdoe@example.com", "12345");

    // Load PGP cert for testing
    let certs = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("crypto")
            .join("cert_pgp.pem"),
    )
    .unwrap();

    let pgp_request = EncryptionType::PGP {
        algo: Algorithm::Aes256,
        certs: certs.clone(),
        allow_spam_training: true,
    };

    // Test 1: Admin can set crypto on another account via ?account=email
    assert_eq!(
        admin_api
            .post::<u32>(
                "/api/account/crypto?account=jane.smith@example.com",
                &pgp_request
            )
            .await
            .unwrap()
            .unwrap_data(),
        1,
        "Admin should be able to set PGP key on jane's account"
    );

    // Test 2: Admin can read crypto config from another account
    let jane_crypto = admin_api
        .get::<EncryptionType>("/api/account/crypto?account=jane.smith@example.com")
        .await
        .unwrap()
        .unwrap_data();
    assert!(
        matches!(jane_crypto, EncryptionType::PGP { .. }),
        "Jane's account should have PGP encryption configured, got: {:?}",
        jane_crypto
    );

    // Test 3: Regular user cannot impersonate (missing Impersonate permission)
    let result = user_api
        .post::<u32>(
            "/api/account/crypto?account=jane.smith@example.com",
            &pgp_request,
        )
        .await
        .unwrap();
    result.expect_request_error("Forbidden");

    // Test 4: Regular user cannot read another account's crypto
    let result = user_api
        .get::<EncryptionType>("/api/account/crypto?account=jane.smith@example.com")
        .await
        .unwrap();
    result.expect_request_error("Forbidden");

    // Test 5: Admin impersonation with non-existent account returns not found
    let result = admin_api
        .post::<u32>(
            "/api/account/crypto?account=nonexistent@example.com",
            &pgp_request,
        )
        .await
        .unwrap();
    let (error, _, _) = result.unwrap_error();
    assert_eq!(
        error, "notFound",
        "Non-existent account should return notFound"
    );

    // Test 6: Admin can disable crypto on another account
    assert_eq!(
        admin_api
            .post::<Option<String>>(
                "/api/account/crypto?account=jane.smith@example.com",
                &EncryptionType::Disabled
            )
            .await
            .unwrap()
            .unwrap_data(),
        None,
        "Admin should be able to disable encryption on jane's account"
    );

    // Test 7: Verify it was actually disabled
    let jane_crypto = admin_api
        .get::<EncryptionType>("/api/account/crypto?account=jane.smith@example.com")
        .await
        .unwrap()
        .unwrap_data();
    assert!(
        matches!(jane_crypto, EncryptionType::Disabled),
        "Jane's encryption should be disabled after admin disabled it"
    );

    // Test 8: Without ?account param, admin manages own account (backward compat)
    assert_eq!(
        admin_api
            .post::<u32>("/api/account/crypto", &pgp_request)
            .await
            .unwrap()
            .unwrap_data(),
        1,
        "Without ?account, admin should manage own account"
    );

    // Clean up: disable admin's own crypto
    admin_api
        .post::<Option<String>>("/api/account/crypto", &EncryptionType::Disabled)
        .await
        .unwrap()
        .unwrap_data();

    // Test 9: Admin sets crypto, then email delivery encrypts for that account
    admin_api
        .post::<u32>(
            "/api/account/crypto?account=jane.smith@example.com",
            &pgp_request,
        )
        .await
        .unwrap()
        .unwrap_data();

    let mut lmtp = SmtpConnection::connect().await;
    lmtp.ingest(
        "bill@example.com",
        &["jane.smith@example.com"],
        concat!(
            "From: bill@example.com\r\n",
            "To: jane.smith@example.com\r\n",
            "Subject: Impersonation test (should be encrypted)\r\n",
            "\r\n",
            "This message should be encrypted at rest."
        ),
    )
    .await;

    // Verify the message was encrypted
    let jane_account = params.account("jane.smith@example.com");
    let jane_client = jane_account.client();
    let mut request = jane_client.build();
    request.get_email();
    let emails = request.send_get_email().await.unwrap().take_list();
    assert!(
        !emails.is_empty(),
        "Jane should have received at least one email"
    );
    let last_email = emails.last().unwrap();
    let message = String::from_utf8(
        jane_client
            .download(last_email.blob_id().unwrap())
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        message.contains("Content-Type: multipart/encrypted"),
        "Email to Jane should be encrypted via admin-provisioned key, got: {}",
        &message[..200.min(message.len())]
    );

    // Clean up
    admin_api
        .post::<Option<String>>(
            "/api/account/crypto?account=jane.smith@example.com",
            &EncryptionType::Disabled,
        )
        .await
        .unwrap()
        .unwrap_data();

    println!("Admin crypto impersonation tests passed!");
}

pub async fn import_certs_and_encrypt() {
    for (name, method, expected_certs) in [
        ("cert_pgp.pem", EncryptionMethod::PGP, 1),
        //("cert_pgp.der", EncryptionMethod::PGP, 1),
        ("cert_smime.pem", EncryptionMethod::SMIME, 3),
        ("cert_smime.der", EncryptionMethod::SMIME, 1),
    ] {
        let mut certs = try_parse_certs(
            method,
            std::fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("resources")
                    .join("crypto")
                    .join(name),
            )
            .unwrap(),
        )
        .expect(name);

        assert_eq!(certs.len(), expected_certs);

        if method == EncryptionMethod::PGP && certs.len() == 2 {
            // PGP library won't encrypt using EC
            let mut certs_ = certs.to_vec();
            certs_.pop();
            certs = certs_.into();
        }

        let mut params = EncryptionParams {
            certs,
            flags: method.flags(),
        };

        for algo in [Algorithm::Aes128, Algorithm::Aes256] {
            let message = MessageParser::new()
                .parse(b"Subject: test\r\ntest\r\n")
                .unwrap();
            assert!(!message.is_encrypted());
            params.flags = algo.flags() | method.flags();
            let arch =
                Archive::deserialize_owned(Archiver::new(params.clone()).serialize().unwrap())
                    .unwrap();
            message
                .encrypt(arch.unarchive::<EncryptionParams>().unwrap())
                .await
                .unwrap();
        }
    }

    // S/MIME and PGP should not be allowed mixed
    assert!(
        try_parse_certs(
            EncryptionMethod::PGP,
            std::fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("resources")
                    .join("crypto")
                    .join("cert_mixed.pem"),
            )
            .unwrap(),
        )
        .is_err()
    );
}

pub fn check_is_encrypted() {
    let messages = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("crypto")
            .join("is_encrypted.txt"),
    )
    .unwrap();

    for raw_message in messages.split("!!!") {
        let is_encrypted = raw_message.contains("TRUE");
        let message = MessageParser::new()
            .parse(raw_message.trim().as_bytes())
            .unwrap();
        assert!(message.content_type().is_some());
        assert_eq!(
            message.is_encrypted(),
            is_encrypted,
            "failed for {raw_message}"
        );
    }
}
