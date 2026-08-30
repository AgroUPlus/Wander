# YouTube Music dans Wander — état des lieux

*Écrit le 2026-08-30. Toutes les mesures datent de ce jour et se re-vérifient avec les sondes
laissées de côté (voir « Reproduire » en fin de document).*

## Ce qui est déjà tranché

- **Plugin de recherche anonyme**, comme Internet Archive : chercher, streamer, télécharger.
  Pas de compte Google, donc pas de likes ni de playlists YouTube.
- **Wander reste MIT.** C'est ce qui a éliminé `rustypipe` (GPL-3.0).
- **Jamendo est supprimé** (commit `848f590`). L'arbre compile, 0 erreur.

## Le point technique à comprendre

YouTube plafonne les clients « non attestés » à **1 Mo par piste**, soit ~1 minute d'audio.
Ce qui lève le plafond est un PO Token (attestation BotGuard), que Wander ne peut pas produire
sans embarquer un moteur JavaScript.

Sauf que l'attestation est appliquée **client par client**, et certains clients de niche y
échappent encore. Résultat des tests :

| Identité déclarée | Résultat |
| :-- | :-- |
| `ANDROID_VR` (casque Quest) | plafonné à 1 Mo — l'identité que wanda utilise |
| `IOS` | idem, sauf sur une piste isolée |
| `rustypipe` (utilise `IOS`/`TV`) | 3 échecs sur 4 |
| **`VISIONOS`** (Apple Vision Pro) | **8 pistes sur 8, intégrales** |

`VISIONOS` n'implique aucun matériel ni logiciel Apple : ce sont six chaînes de caractères dans
le corps JSON et les en-têtes. Wanda connaît déjà cette identité mais la réserve aux
livestreams — elle ne l'avait pas généralisée aux pistes ordinaires.

**Ce que ça achète :** Rust pur, MIT, un seul binaire, aucun moteur JS, aucune dépendance GPL,
aucun binaire externe (yt-dlp devient inutile).

**Ce que ça coûte :** c'est un trou dans une politique, pas une API publique. YouTube peut le
fermer, comme il vient de le faire pour `ANDROID_VR`. Retombée douce cependant : seule la
fonction de résolution du flux serait à re-pointer, ~50 lignes isolées.

## Recette de lecture

1. `POST music.youtube.com/youtubei/v1/visitor_id` (contexte `WEB_REMIX`) → `visitorData`
   anonyme, mis en cache pour la durée du processus. Sans lui, `VISIONOS` répond
   LOGIN_REQUIRED ; un UUID fabriqué localement ne suffit pas.
2. `POST www.youtube.com/youtubei/v1/player` en contexte `VISIONOS` avec ce `visitorData` :
   `clientVersion 1.02`, `deviceModel RealityDevice17,1`, `osName visionOS`,
   `osVersion 26.5.23O471`. En-têtes `X-YouTube-Client-Name: 101`, `X-Goog-Visitor-Id`.
3. Prendre le meilleur `adaptiveFormats` audio (Opus itag 251, AAC 140 en repli). L'`url` est
   en clair : ni `signatureCipher`, ni paramètre `n`.

**Trois exigences de transport.** Chacune donne un 403 silencieux si elle manque :

- requêtes **`Range`** obligatoires (tranches de 512 Ko validées) ;
- le **User-Agent VISIONOS sur chaque requête média**, pas seulement sur `/player` ;
- **suivre les redirections 302**.

## Le travail, par ordre

1. **Vérifier d'abord** que le lecteur émet des requêtes `Range` (`src/player/mod.rs:827`).
   Si non, c'est le premier chantier — tout le reste en dépend.
2. `src/plugins/ytmusic/` : `variant.rs`, `params.rs`, `api.rs`, `parse.rs`, `ui.rs`. Le gros
   du travail est le parsing InnerTube (~450 lignes chez wanda), transposable presque
   directement — c'est du JSON pur, sans dépendance Android.
3. Câblage `OnlineSource` : ~16 points d'accroche, tous énumérés dans le plan détaillé. Le
   code de Jamendo dans l'historique git en donne le gabarit exact.
4. **Id stable `ytm:<videoId>`** avec résolution paresseuse dans `MergedLibrary::open`
   (`src/library/merged.rs:105`). Non négociable : une URL googlevideo est liée à l'IP et
   expire, donc inutilisable en jam ou dans une file sauvegardée.
5. **Jams** : `follow_jam_now_playing` (`src/app/jam.rs:296`) ne résout aujourd'hui que par
   correspondance titre/artiste sur ce qu'on possède déjà. Sans une passe `ytm:` supplémentaire,
   une piste YouTube jouée par un pair serait simplement sautée. À placer **après** les
   recherches locales, pour qu'un morceau possédé reste servi depuis le disque.
6. Corriger `SongSource::of()` (`src/library/mod.rs:70`) : un id `ytm:` retombe aujourd'hui sur
   `Server` et afficherait « Navidrome » sous une piste YouTube.

## À décider demain

- Faut-il un repli automatique vers une autre identité cliente quand `VISIONOS` tombe, ou un
  message d'erreur franc et une mise à jour de constante ?
- Le téléchargement (touche `d`) est-il souhaitable ici, ou seulement le streaming ?
- Jam Wander ↔ Wanda : wanda emploie déjà le préfixe `ytm:` et `agro.rs:78` le réserve. À
  tester ensemble une fois le plugin debout.

## Reproduire

Sondes dans le scratchpad de session :
`scratchpad/ytm/visionos.py` (le test décisif, 8 pistes), `scratchpad/ytm/probe*.py` (mesures
du plafond), `scratchpad/rp_probe/` (essai rustypipe).

Plan détaillé complet : `~/.claude/plans/a-quel-point-serait-ce-glowing-rossum.md`.

## Convention à tenir

Reprendre les commentaires de wanda en les traduisant, pas seulement son code : chaque
constante y porte **la panne qu'elle corrige et sa date de vérification**. Rien de tout cela
n'est documenté ni stable côté YouTube — c'est la seule défense contre une rupture silencieuse.
