/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::{JMAPTest, mail::delivery::SmtpConnection, wait_for_index};
use jmap_client::email;
use jmap_client::core::query::Filter;

/// Regression test for `hasAttachment` filter in Email/query.
///
/// The search index was determining `hasAttachment` by checking whether the
/// Attachment TEXT field had been populated. Binary attachments (PDFs, images)
/// have no extractable text, so `hasAttachment` was false even though the
/// email had attachments. The fix uses the metadata flag computed during
/// ingestion (which correctly detects all attachment types) instead.
pub async fn test(params: &mut JMAPTest) {
    println!("Running hasAttachment filter tests...");

    let server = params.server.clone();
    let account = params.account("jdoe@example.com");
    let client = account.client();

    // -------------------------------------------------------------------------
    // Ingest a plain text email (no attachment)
    // -------------------------------------------------------------------------
    let mut lmtp = SmtpConnection::connect().await;
    lmtp.ingest(
        "sender@example.com",
        &["jdoe@example.com"],
        concat!(
            "From: sender@example.com\r\n",
            "To: jdoe@example.com\r\n",
            "Subject: Plain text email\r\n",
            "Message-ID: <plain-text-no-attach@test>\r\n",
            "Date: Mon, 09 Mar 2026 08:00:00 +0100\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "This is a plain text email with no attachments.\r\n",
        ),
    )
    .await;

    // -------------------------------------------------------------------------
    // Ingest an email with a binary attachment (PNG image, base64-encoded)
    // -------------------------------------------------------------------------
    lmtp.ingest(
        "sender@example.com",
        &["jdoe@example.com"],
        concat!(
            "From: sender@example.com\r\n",
            "To: jdoe@example.com\r\n",
            "Subject: Email with binary attachment\r\n",
            "Message-ID: <binary-attachment@test>\r\n",
            "Date: Mon, 09 Mar 2026 09:00:00 +0100\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=\"boundary42\"\r\n",
            "\r\n",
            "--boundary42\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "This email has a binary PNG attachment.\r\n",
            "\r\n",
            "--boundary42\r\n",
            "Content-Type: image/png; name=\"pixel.png\"\r\n",
            "Content-Disposition: attachment; filename=\"pixel.png\"\r\n",
            "Content-Transfer-Encoding: base64\r\n",
            "\r\n",
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4\r\n",
            "nGNgYPgPAAEEAQBwIGUDAAAAAElFTkSuQmCC\r\n",
            "\r\n",
            "--boundary42--\r\n",
        ),
    )
    .await;

    lmtp.quit().await;

    // Wait for indexing to complete
    wait_for_index(&server).await;

    // -------------------------------------------------------------------------
    // Query: hasAttachment = true → must find the binary attachment email
    // -------------------------------------------------------------------------
    let with_attach = client
        .email_query(
            Filter::has_attachment(true).into(),
            None::<Vec<email::query::Comparator>>,
        )
        .await
        .unwrap();
    assert!(
        !with_attach.ids().is_empty(),
        "hasAttachment:true should find the email with a binary PNG attachment"
    );

    // -------------------------------------------------------------------------
    // Query: hasAttachment = false → must find the plain text email
    // -------------------------------------------------------------------------
    let without_attach = client
        .email_query(
            Filter::has_attachment(false).into(),
            None::<Vec<email::query::Comparator>>,
        )
        .await
        .unwrap();
    assert!(
        !without_attach.ids().is_empty(),
        "hasAttachment:false should find the plain text email"
    );

    // -------------------------------------------------------------------------
    // The two sets should be disjoint
    // -------------------------------------------------------------------------
    let with_ids: std::collections::HashSet<&str> =
        with_attach.ids().iter().map(|s| s.as_str()).collect();
    let without_ids: std::collections::HashSet<&str> =
        without_attach.ids().iter().map(|s| s.as_str()).collect();
    assert!(
        with_ids.is_disjoint(&without_ids),
        "hasAttachment:true and hasAttachment:false should return disjoint sets"
    );

    // Cleanup
    for id in with_attach.ids() {
        client.email_destroy(id).await.unwrap();
    }
    for id in without_attach.ids() {
        client.email_destroy(id).await.unwrap();
    }

    println!("hasAttachment filter tests passed ✓");
}
