# Stalwart Bugs & Limitations — Impact Activate Workspace

Audit des bugs et limitations Stalwart 0.15.5 qui impactent Activate Workspace (client JMAP pur).
Dernière mise à jour : 2026-03-04.

## 🔴 Critiques (fixés ou contournés lourdement)

### 1. Send-to-self JMAP — FIXÉ (fork)

- **Issue** : [Discussion #2705](https://github.com/stalwartlabs/stalwart/discussions/2705)
- **Problème** : `EmailSubmission/set` vers soi-même → email silencieusement droppé par la dedup
- **Root cause** : `email_ingest()` dans `ingest.rs` ne vérifie que `INBOX_ID`/`JUNK_ID`, pas `SENT_ID`/`DRAFTS_ID`
- **Fix** : Branche `fix/jmap-send-to-self` déployée en prod
- **Détails** : voir `CLAUDE.md`

### 2. Pas de JMAP Tasks (VTODO)

- **Problème** : Stalwart expose les VTODO via CalDAV uniquement, pas de méthode JMAP `Task/get`, `Task/set`, etc.
- **Impact** : Le module Tasks utilise CalDAV (Basic auth, PROPFIND/REPORT/PUT/DELETE) au lieu de JMAP
- **Conséquences** : auth séparée, pas de push temps réel, pas d'intégration JMAP native
- **Workaround** : `lib/caldav/client.ts` + `stores/task-store.ts` avec CalDAV
- **Forkable** : Non — il faudrait écrire une spec JMAP Tasks custom, trop gros

### 3. EventSource Bearer auth → 403 — FIXÉ (fork)

- **Problème** : EventSource (SSE) pour push JMAP nécessite Bearer token en query param, rejeté par Stalwart
- **Conformance** : Test `push-eventsource/eventsource-receives-state-change` FAIL
- **Impact** : Pas de notifications temps réel, fallback polling 15s
- **Root cause** : `crates/http/src/request.rs` — le handler EventSource n'appelle que `authenticate_headers()`, pas de fallback `?access_token=` (RFC 6750 §2.3)
- **Fix** : Branche `fix/eventsource-query-auth`, fallback sur `?access_token=` avec `Credentials::OAuthBearer`. Voir `CLAUDE.md` pour les détails.

### 4. PushSubscription non implémenté

- **Problème** : 7/7 tests PushSubscription FAIL (`Unknown method 'set'`)
- **Impact** : Pas de web push natif JMAP
- **Workaround** : Push via Laravel backend séparé (`PushController` + `web-push` VAPID)
- **Forkable** : Non — feature complète à implémenter

## 🟡 Pénibles (workarounds en place)

### 5. `recurrenceRule` singulier (non-standard)

- **Problème** : Stalwart utilise `recurrenceRule` (objet) au lieu de `recurrenceRules` (array, RFC 8984 JSCalendar)
- **Impact** : Tout le code calendrier utilise le singulier, incompatible avec d'autres serveurs JMAP
- **Fichiers** : `lib/jmap/types.ts`, `lib/calendar/ics.ts`, `components/calendar/event-form.tsx`

### 6. FileNode — 6 quirks — 5 FIXÉS (fork)

Tous dans `lib/jmap/modules/filestorage.ts` :

| Quirk | Workaround | Status |
|---|---|---|
| Capability URI non-standard (`jmap:filenode` vs `jmap:filestorage`) | Hardcodé `CAP = "urn:ietf:params:jmap:filenode"` | ✅ FIXÉ (`fix/filenode-capability-uri`) |
| `parentId: null` filter rejeté (impossible de query la racine) | Fetch ALL nodes, filtre client-side | ✅ FIXÉ (`fix/filenode-parentid-null-filter`) |
| Folder creation casse si on envoie `type:"directory"` + `blobId` | Créer avec juste `{ name }` | ✅ FIXÉ (`fix/filenode-folder-type-directory`) |
| Props par défaut incomplètes (manque `type`, `blobId`, `created`, `modified`) | Request explicite de toutes les props | ✅ FIXÉ (`fix/filenode-default-properties`) |
| Folders retournent `type:null, blobId:null, size:null` | Normalisation client (detect folder par absence de `blobId`) | Comportement normal (dossier = pas de fichier) |
| `contentType` rejeté sur `FileNode/set` | Omis des appels create | ✅ FIXÉ (`fix/filenode-contenttype-alias`) |

### 7. FileNode/set update `blobId` non fiable

- **Problème** : Mettre à jour le `blobId` d'un FileNode existant ne fonctionne pas
- **Impact** : Preferences sync (`.activate-preferences.json` dans le Drive)
- **Workaround** : Delete + recreate à chaque save (`lib/jmap/modules/preferences.ts`)

### 8. Identity/get ordering aléatoire

- **Problème** : Identités retournées dans l'ordre de création, pas par priorité domaine
- **Impact** : L'identité alias peut apparaître avant la primaire
- **Workaround** : Tri client-side dans `auth-store.ts` par domaine OAuth login

### 9. `notFound` arrays vides partout — FIXÉ (fork)

- **Issue** : [#2835](https://github.com/stalwartlabs/stalwart/issues/2835) (JMAP conformance)
- **Problème** : Quand on request un ID inexistant, `notFound` est vide au lieu de contenir l'ID
- **Root cause** : IDs invalides (hors alphabet base32) silencieusement droppés au parsing, IDs valides-mais-inexistants jamais ajoutés à `notFound`
- **Fix** : Branche `fix/jmap-notfound-ids` déployée en prod
- **Détails** : voir `C:\Projets\stalwart\CLAUDE.md`

### 10. `shareWith mayReadItems:false` fait disparaître le calendrier

- **Problème** : Mettre `mayReadItems: false` sur un calendrier partagé le supprime de la session JMAP du destinataire
- **Impact** : Même le niveau "freebusy" nécessite `mayReadItems: true`
- **Workaround** : Toujours `mayReadItems: true`, privacy gérée via le champ `privacy` des events
- **Fichier** : `components/calendar/permission-select.tsx`

### 11. Shared calendar ID collisions

- **Problème** : IDs JMAP per-account, pas globalement uniques → un calendrier partagé peut avoir le même bare ID qu'un calendrier personnel
- **Workaround** : Composite keys `shared:${accountId}:${id}` via `calendarKey()` dans `calendar-store.ts`

### 12. Draft destroy obligatoire avant re-send

- **Problème** : Update keywords d'un draft existant + submit ne fonctionne pas (body/attachments non inclus)
- **Workaround** : Toujours destroy l'ancien draft et créer un email frais avec body complet
- **Fichier** : `lib/jmap/modules/draft-send.ts`

### 13. Principal/query filter incompatible

- **Problème** : Le filtre `text` standard ne fonctionne pas toujours sur `Principal/query`
- **Workaround** : Triple fallback (text → OR name/email → fetch all + filtre client)
- **Fichier** : `lib/jmap/modules/sharing.ts`

## 🟢 Limitations tolérées

### 14. VacationResponse init buggé

- **Conformance** : 6/6 tests vacation FAIL
- **Workaround** : `try/catch` silencieux dans `lib/jmap/modules/vacation.ts`, page settings peut être vide

### 15. Admin API SET instable

- **Problème** : `PATCH /api/principal/...` ne marche pas toujours en v0.15.x
- **Workaround** : Éditer `config.toml` via SSH + `systemctl restart stalwart-mail`

### 16. `password_grant` impossible

- **Problème** : Stalwart 0.15.5 ne supporte que `authorization_code` et `device_code`
- **Impact** : Pas de login programmatique headless
- **Workaround** : `authorization_code` exclusivement

### 17. `hasAttachment` filter cassé — FIXÉ (fork)

- **Conformance** : Tests `email/filter-has-attachment-true` et `false` FAIL
- **Impact** : Recherche emails avec/sans pièces jointes retourne des résultats incorrects
- **Root cause** : `search.rs` détermine `has_attachment` par présence de texte extrait des PJ. Les PJ binaires (PDF, images) n'ont pas de texte → `false` alors qu'il y a des PJ.
- **Fix** : Branche `fix/hasattachment-filter`, utilise le flag `MESSAGE_HAS_ATTACHMENT` de la métadonnée. Voir `CLAUDE.md` pour les détails.

### 18. Anchor-based pagination cassée — FIXÉ (fork)

- **Conformance** : Tests `email/paging-anchor*` FAIL (retourne 0 résultats)
- **Impact** : Scroll-to-email par anchor ne fonctionnerait pas
- **Root cause** : `QueryResponseBuilder` — position non trackée en mode anchor, off-by-one pour offset négatif, paramètre `position` non ignoré quand `anchor` est présent (RFC 8620 §5.5)
- **Fix** : Branche `fix/anchor-pagination` déployée en prod. 11 tests unitaires ajoutés.
- **Détails** : voir `CLAUDE.md`

### 19. CalDAV URL encoding usernames

- **Problème** : Stalwart peut URL-encoder le username dans les hrefs CalDAV (`jf%40activate.ch`)
- **Workaround** : Double check raw + encoded dans `task-store.ts`

### 20. Admin API PATCH format array

- **Problème** : Format `[{"action":"set","field":"...","value":"..."}]` (pas JSON objet)
- **Workaround** : Documenté, tous les appels admin utilisent le format array

## ✅ Bugs résolus (v0.15.5)

### collapseThreads sort — RÉSOLU

- **Issue** : [#224](https://github.com/stalwartlabs/stalwart/issues/224) (ouverte depuis v0.5.x, janvier 2024)
- **Problème historique** : `Email/query` avec `collapseThreads=true` et un seul `sort` comparateur ne triait pas du tout
- **Root cause (v0.5.x)** : 3 variantes dans `Store::sort()`, la 3ème (fallback) ne triait pas
- **Résolution** : Refactoring complet en v0.15.x — sort séparé de collapseThreads (post-filtre sur résultats déjà triés)
- **Vérifié en prod** : 2026-03-04, tri `receivedAt` desc correct avec `collapseThreads=true`

## Conformance JMAP

Source : [jmap-test-suite](https://github.com/jmapio/jmap-test-suite) par josephg ([rapport](https://seph.au/jmap-report.html))

- **Stalwart 0.15.x** : 264/309 tests passent (85.4%)
- **45 failures** dont ~20 impactent directement un client JMAP
- **Issue upstream** : [#2835](https://github.com/stalwartlabs/stalwart/issues/2835)
