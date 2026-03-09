# CLAUDE.md — Stalwart Fork (silbox-ch)

## Projet

Fork de [stalwartlabs/stalwart](https://github.com/stalwartlabs/stalwart) pour corriger des bugs critiques bloquants pour Activate Workspace (client JMAP pur).

- **Fork** : https://github.com/silbox-ch/stalwart
- **Upstream** : https://github.com/stalwartlabs/stalwart
- **Licence** : AGPL-3.0-only (FLA requis pour contribuer upstream)
- **Local** : `C:/Projets/stalwart/`
- **Serveur** : `/opt/stalwart-build/` (source) + `/opt/stalwart/bin/stalwart` (binaire déployé)

## Branches

| Branche | Description | Status |
|---|---|---|
| `production` | Merge des 10 fix branches → branche de compilation | **Déployé en prod** |
| `fix/jmap-send-to-self` | Fix dedup send-to-self via JMAP | Mergé dans `production`, Discussion #2705 postée |
| `fix/jmap-notfound-ids` | Retourner les IDs invalides dans notFound (RFC 8620 §5.1) | Mergé dans `production`, **en attente réponse #2705 avant de commenter #2835** |
| `fix/eventsource-query-auth` | Support Bearer token via `?access_token=` pour EventSource SSE | Mergé dans `production`, déployé |
| `fix/hasattachment-filter` | Fix `hasAttachment` filter pour pièces jointes binaires | Mergé dans `production`, déployé |
| `fix/filenode-capability-uri` | Rename `jmap:filenode` → `jmap:filestorage` (IETF convention) | Mergé dans `production` |
| `fix/filenode-default-properties` | Ajout BlobId, Type, Created, Modified aux props par défaut | Mergé dans `production` |
| `fix/filenode-contenttype-alias` | Alias `contentType` → `Type` en désérialisation | Mergé dans `production` |
| `fix/filenode-parentid-null-filter` | Support `parentId:null` dans FileNode/query (root nodes) | Mergé dans `production` |
| `fix/filenode-folder-type-directory` | `type:"directory"` crée un dossier (file=None) | Mergé dans `production` |
| `fix/anchor-pagination` | Fix anchor-based pagination position + off-by-one (RFC 8620 §5.5) | Mergé dans `production` |

## Bug fix: JMAP send-to-self (Discussion #2705)

### Problème

Envoyer un email à soi-même via JMAP `EmailSubmission/set` échoue silencieusement. L'email arrive dans Sent mais jamais dans Inbox.

### Root cause

`email_ingest()` dans `crates/email/src/message/ingest.rs` : la logique de déduplication trouve le `Message-ID` déjà en Sent (placé par `onSuccessUpdateEmail` AVANT la livraison async) et le considère comme doublon car elle ne vérifie que `INBOX_ID` et `JUNK_ID`.

### Fix

- **`crates/email/src/message/ingest.rs`** : Ajout d'un check `in_sent_or_drafts` — si le doublon n'existe que dans Sent/Drafts, c'est un self-send légitime → livrer quand même.
- **`tests/src/jmap/mail/delivery.rs`** : Test de régression (import dans Sent + livraison LMTP, assert Inbox count augmente).

### Pourquoi IMAP/SMTP fonctionne

Thunderbird copie dans Sent APRÈS la livraison SMTP, donc le `Message-ID` n'existe pas encore pendant `email_ingest()`. Avec JMAP, `onSuccessUpdateEmail` déplace dans Sent AVANT la livraison async.

### Mailbox constants (référence)

```rust
// crates/email/src/mailbox/mod.rs
pub const INBOX_ID: u32 = 0;
pub const TRASH_ID: u32 = 1;
pub const JUNK_ID: u32 = 2;
pub const DRAFTS_ID: u32 = 3;
pub const SENT_ID: u32 = 4;
```

## Bug fix: JMAP notFound arrays (Issue #2835)

### Problème

Les méthodes `Foo/get` (Email/get, Mailbox/get, Thread/get, etc.) retournent un array `notFound` vide quand on envoie des IDs contenant des caractères invalides pour l'encodage base32 de Stalwart. La RFC 8620 §5.1 exige que TOUT ID demandé mais non trouvé apparaisse dans `notFound`.

### Root cause

Stalwart utilise un base32 custom (alphabet `abcdefghijklmnopqrstuvwxyz792013`). Les caractères `-`, `_`, digits `4/5/6/8` font échouer `Id::from_str()`. Lors de la désérialisation, ces IDs deviennent `MaybeIdReference::Invalid(s)`, puis `into_valid()` les drop silencieusement via `filter_map(try_unwrap)`. Le JMAP test suite envoie des IDs comme `"nonexistent-email-xyz"` → silently dropped → jamais dans `notFound`.

### Fix

- **`crates/jmap-proto/src/method/get.rs`** : Nouveau type `NotFoundIds<I>` qui collecte les IDs valides-mais-introuvables ET les chaînes non-parseables. Sérialise en un seul array JSON plat. Modification de `unwrap_ids()` pour séparer les IDs invalides au lieu de les dropper.
- **`crates/jmap-proto/src/object/calendar_event_notification.rs`** : `CalendarEventNotificationGetResponse.not_found` passe de `Vec<Id>` à `NotFoundIds<Id>`.
- **16 handlers `Foo/get`** : Destructuration du tuple `(ids, not_found_ids)` + appel `response.not_found.add_invalid(not_found_ids)`.
- **`tests/src/jmap/mail/not_found.rs`** : Test de régression (5 scénarios, 4 méthodes JMAP).

### Tests confirmés

- Compilation release : ✅
- Test `notFound compliance tests passed` : ✅
- Déployé en prod : ✅

### Action en attente

Commenter l'issue #2835 avec le fix une fois que mdecimus aura répondu à la Discussion #2705 (send-to-self). Ne pas spammer avant.

## Bug fix: EventSource query parameter auth

### Problème

Le endpoint `/jmap/eventsource` ne supporte que l'authentification via le header HTTP `Authorization`. L'API `EventSource` des navigateurs ne permet pas d'envoyer des headers custom → les clients JMAP browser reçoivent 403.

### Root cause

`crates/http/src/request.rs` : le handler EventSource appelle uniquement `authenticate_headers()` qui lit le header `Authorization`. Pas de fallback sur `?access_token=` comme le prévoit la RFC 6750 §2.3.

### Fix

- **`crates/http/src/request.rs`** : Fallback sur `?access_token=` si `authenticate_headers()` échoue avec `AuthEvent::Failed`. Construit `Credentials::OAuthBearer` et appelle `self.authenticate()` directement.
- **`tests/src/jmap/auth/oauth.rs`** : 3 tests de régression (token valide → 200, pas d'auth → 401, token invalide → 401).

### Impact

Active Workspace peut maintenant utiliser `EventSource` pour le push temps réel au lieu du polling 15s.

## Bug fix: hasAttachment filter

### Problème

`Email/query` avec le filtre `hasAttachment: true` ne trouvait pas les emails avec des pièces jointes binaires (PDF, images). Les tests JMAP `email/filter-has-attachment-true` et `false` FAIL.

### Root cause

`crates/email/src/message/index/search.rs:342-345` : `has_attachment` est déterminé par `document.has_field(Attachment)` qui vérifie si du **texte** a été extrait des pièces jointes. Les pièces jointes binaires (PDF, images) n'ont pas de texte extractible → `has_field` retourne `false` → l'index dit "pas d'attachement" alors qu'il y en a un.

La métadonnée correcte (`MESSAGE_HAS_ATTACHMENT`) est déjà calculée dans `metadata.rs` pendant l'ingestion (détecte Binary, Message, et Text/HTML non-body) mais n'était pas utilisée pour l'index de recherche.

### Fix

- **`crates/email/src/message/index/search.rs`** : Remplacé `document.has_field(Attachment)` par `(self.rcvd_attach.to_native() & MESSAGE_HAS_ATTACHMENT) != 0`.
- **`tests/src/jmap/mail/has_attachment.rs`** : Test de régression (email plain + email avec PNG binaire, vérifie hasAttachment:true/false).

### Note

Les emails déjà indexés avant le fix gardent l'ancien index. Un reindex serait nécessaire pour les corriger, mais les nouveaux emails sont correctement indexés.

## Bug fixes: FileNode (5 fixes)

Série de 5 correctifs pour l'implémentation JMAP FileNode (file storage). Chaque fix est sur sa propre branche pour soumission individuelle upstream.

### Fix 1 — Capability URI (`fix/filenode-capability-uri`)

- **Problème** : URI `urn:ietf:params:jmap:filenode` non-standard (convention IETF = `filestorage`)
- **Fix** : Serialize → `filestorage`, deserialize accepte les deux pour backward compat
- **Fichiers** : `crates/jmap-proto/src/request/capability.rs`, `tests/src/jmap/principal/get.rs`

### Fix 2 — parentId:null filter (`fix/filenode-parentid-null-filter`)

- **Problème** : `FileNode/query` avec `parentId: null` échoue (désérialisation attend string, pas null)
- **Root cause** : `MaybeInvalid<Id>` désérialise via `<&str>::deserialize()` → null = erreur
- **Fix** : Map `parentId: null` → `HasParentId(false)` (nœuds racine)
- **Fichier** : `crates/jmap-proto/src/object/file_node.rs`

### Fix 3 — Folder creation avec type:directory (`fix/filenode-folder-type-directory`)

- **Problème** : Envoyer `type: "directory"` crée un `FileProperties` avec media_type "directory" au lieu d'un dossier
- **Root cause** : `file_node.file.get_or_insert_default()` crée un `Some(FileProperties)` — le serveur traite ça comme un fichier
- **Fix** : Intercepter `"directory"` et setter `file_node.file = None` (= dossier)
- **Fichier** : `crates/jmap/src/file/set.rs`

### Fix 4 — Default properties (`fix/filenode-default-properties`)

- **Problème** : `FileNode/get` sans `properties` retourne seulement Id, Name, ParentId, Size — manque BlobId, Type, Created, Modified
- **Fix** : Ajout des 4 propriétés manquantes au tableau de defaults
- **Fichier** : `crates/jmap/src/file/get.rs`

### Fix 6 — contentType alias (`fix/filenode-contenttype-alias`)

- **Problème** : Les clients JMAP utilisent couramment `contentType` pour le MIME type, mais seul `type` est accepté
- **Fix** : Ajout `contentType` comme alias de désérialisation → `FileNodeProperty::Type`
- **Fichier** : `crates/jmap-proto/src/object/file_node.rs`

## Bug fix: Anchor-based pagination (RFC 8620 §5.5)

### Problème

`Foo/query` avec `anchor` + `anchorOffset` retourne des résultats incorrects. Les tests JMAP `email/paging-anchor*` FAIL (0 résultats). Trois bugs dans `QueryResponseBuilder` :

1. **Position ignorée avec anchor** : Le paramètre `position` de la requête initialisait le compteur interne au lieu d'être ignoré quand `anchor` est présent (RFC §5.5).
2. **Position non trackée** : `response.position` valait toujours 0 au lieu de l'index réel du premier résultat.
3. **Off-by-one (offset négatif)** : `anchorOffset=-N` retournait N items finissant par l'anchor, au lieu de N items *avant* l'anchor.

### Root cause

`crates/jmap/src/api/query.rs` — `QueryResponseBuilder` :

- `position` initialisé à `request.position.unwrap_or(0)` même en mode anchor (devrait être `0`)
- Positive offset path : pas d'incrémentation de `self.position` pour les items skippés avant/après l'anchor
- Negative offset `build()` : `start_offset = ids.len() - position` au lieu de `ids.len() - 1 - position` (anchor_index vs ids.len())

### Fix

- **`crates/jmap/src/api/query.rs`** :
  - `position: if has_anchor { 0 } else { request.position.unwrap_or(0) }`
  - `self.position += 1` dans le positive offset path (items avant anchor + anchor_offset skip)
  - `anchor_index = ids.len().saturating_sub(1)` + `start_offset = anchor_index.saturating_sub(position)` dans build()
- **Tests unitaires** : 11 cas couvrant offset positif, négatif, clamping, anchor not found, unlimited, position-ignored-with-anchor.

## Déploiement

### Binaire actuel

- **Version** : 0.15.5 (code upstream main + 10 fixes)
- **Commit** : `721f4ef0` (branche `production`, merge des 10 fix branches)
- **Compilé** : 2026-03-09
- **Binaire** : `/opt/stalwart/bin/stalwart` (64MB, compilé depuis le fork)
- **Backup officiel** : `/opt/stalwart/bin/stalwart.0.15.5.official` (80MB)
- **Service** : `systemctl restart stalwart-mail`

### Compiler et déployer

```bash
# Sur le serveur hub.activate.ch
ssh -i C:/Projets/activate-hub/id_ed25519 root@82.197.186.110

# Recompiler après merge/rebase
cd /opt/stalwart-build
git fetch origin
git reset --hard origin/production
source ~/.cargo/env
cargo build --release -p stalwart
# ~7 min (LTO mono-thread pour le linking final)

# Déployer
systemctl stop stalwart-mail
cp target/release/stalwart /opt/stalwart/bin/stalwart
chmod +x /opt/stalwart/bin/stalwart
systemctl start stalwart-mail

# Nettoyer les artefacts (2GB)
rm -rf target/
```

### Rollback vers l'officiel

```bash
systemctl stop stalwart-mail
cp /opt/stalwart/bin/stalwart.0.15.5.official /opt/stalwart/bin/stalwart
systemctl start stalwart-mail
```

### Rebase upstream

```bash
# En local — rebase chaque branche de fix, puis recréer production
cd C:/Projets/stalwart
git fetch upstream

# Rebase chaque fix branch
for branch in fix/jmap-send-to-self fix/jmap-notfound-ids fix/eventsource-query-auth fix/hasattachment-filter fix/filenode-capability-uri fix/filenode-default-properties fix/filenode-contenttype-alias fix/filenode-parentid-null-filter fix/filenode-folder-type-directory fix/anchor-pagination; do
  git checkout $branch
  git rebase upstream/main
  git push origin $branch --force-with-lease
done

# Recréer la branche production (merge des 10 fix branches)
git checkout -B production upstream/main
git merge --no-edit fix/jmap-send-to-self
git merge --no-edit fix/jmap-notfound-ids
git merge --no-edit fix/eventsource-query-auth
git merge --no-edit fix/hasattachment-filter
git merge --no-edit fix/filenode-capability-uri
git merge --no-edit fix/filenode-default-properties
git merge --no-edit fix/filenode-contenttype-alias
git merge --no-edit fix/filenode-parentid-null-filter
git merge --no-edit fix/filenode-folder-type-directory
git merge --no-edit fix/anchor-pagination
git push origin production --force-with-lease

# Puis recompiler sur le serveur (voir ci-dessus)
```

## Toolchain serveur

- **Rust** : 1.93.1 (`source ~/.cargo/env` pour activer)
- **Dépendances** : cmake, pkg-config, libssl-dev, build-essential, protobuf-compiler, clang
- **Features Cargo** : `default = ["rocks", "enterprise"]` (RocksDB + enterprise)
- **Source** : `/opt/stalwart-build/` (shallow clone de la branche fix)

## Contribution upstream

- **Discussion** : https://github.com/stalwartlabs/stalwart/discussions/2705#discussioncomment-15996610
- **PRs** : Bloquees pour l'instant (permissions CreatePullRequest refusees). teal-bauer (#2828) avait probablement ete autorise manuellement par mdecimus
- **FLA** : Signature automatique via CLA Assistant bot lors de l'ouverture d'une PR
- **Convention** : PAS de `Co-Authored-By: Claude` dans les commits
- **Issue umbrella** : #2835 (JMAP conformance) — notre porte d'entree pour les fixes
- **Plan detaille** : voir `CONTRIBUTION-PLAN.md`

## Git remotes

```
origin    https://github.com/silbox-ch/stalwart.git (fork)
upstream  https://github.com/stalwartlabs/stalwart.git (upstream)
```

## Audit bugs & limitations

Voir **`STALWART-BUGS.md`** pour l'audit complet (20 bugs/limitations documentés, classés par sévérité).

Résumé :
- 🔴 4 critiques (2 fixés par notre fork, 2 contournés)
- 🟢 `hasAttachment` filter fixé (bug #17)
- 🟡 9 pénibles (workarounds en place dans Activate Workspace)
- 🟢 7 tolérés (impact faible ou pas exposé dans l'UI)
- ✅ 1 résolu naturellement (collapseThreads sort, corrigé par refactoring v0.15.x)

### Prochains candidats pour le fork

| Priorité | Bug | Effort | Status |
|---|---|---|---|
| ~~2~~ | ~~`notFound` arrays vides (error handling)~~ | ~~Small~~ | ✅ Fixé et déployé |
| ~~1~~ | ~~EventSource Bearer auth → 403 (push temps réel)~~ | ~~Moyen~~ | ✅ Fixé et déployé |
| ~~3~~ | ~~`hasAttachment` filter cassé (recherche)~~ | ~~Small~~ | ✅ Fixé et déployé |
| ~~4~~ | ~~FileNode 5 quirks (capability URI, parentId:null, folder type, default props, contentType)~~ | ~~Small~~ | ✅ 5 fixes déployés |
| ~~5~~ | ~~Anchor-based pagination (RFC 8620 §5.5)~~ | ~~Small~~ | ✅ Fixé et déployé |
| — | JMAP Tasks | Trop gros (spec custom nécessaire) | — |
