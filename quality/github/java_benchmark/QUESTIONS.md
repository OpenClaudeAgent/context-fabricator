# Java benchmark questions

Each bilingual pair represents one intent. The benchmark replays each pinned release tree as the active branch of a local fixture. Apache Commons CLI is the calibration set. Jackson Databind is the held-out reference.

This revision incorporates an independent, result-blind review of wording and required evidence.

## apache/commons-cli · 8 intents

Revision [`d74613321325`](https://github.com/apache/commons-cli/tree/d74613321325f6081fa0206c4ab608ab65d80e2e) · [License](https://github.com/apache/commons-cli/blob/d74613321325f6081fa0206c4ab608ab65d80e2e/LICENSE.txt) · [Exact source judgments](commons-cli-gold.json)

### long-option-default

- **EN** Does DefaultParser accept an unambiguous abbreviation for a long option by default, and what matching behavior remains when that is disabled?
- **FR** Par défaut, DefaultParser accepte-t-il une abréviation non ambiguë d’une option longue, et quel type de recherche reste possible si l’on désactive ce comportement ?

### option-terminator

- **EN** Which standalone token makes the remaining command-line tokens positional arguments, and where is that token defined?
- **FR** Quel jeton isolé transforme les arguments suivants en arguments positionnels, et où sa valeur est-elle définie ?

### unexpected-switch

- **EN** Contrast THROW and IGNORE when DefaultParser sees an unknown hyphen-prefixed option longer than one character.
- **FR** Comment les comportements THROW et IGNORE diffèrent-ils quand DefaultParser rencontre une option inconnue commençant par un tiret et de longueur supérieure à un ?

### default-flag-values

- **EN** Which values in the properties file make an argumentless default option present?
- **FR** Quelles valeurs du fichier de propriétés activent par défaut une option qui ne prend pas d’argument ?

### default-precedence

- **EN** When a caller supplies an option explicitly and a default for that same option, which value wins? Can a default choose another member of its already selected option group?
- **FR** Une option est fournie explicitement et a aussi une valeur par défaut. Laquelle prévaut ? Une valeur par défaut peut-elle sélectionner un autre membre du groupe déjà choisi ?

### quote-defaults

- **EN** With quote stripping left at its default, how does the parser handle a surrounding pair of double quotes, with no interior double quotes, in the next argument token versus a property default?
- **FR** Avec le réglage par défaut, comment le parseur traite-t-il une paire de guillemets doubles sans autre guillemet à l’intérieur, dans le jeton d’argument suivant puis dans une valeur par défaut des propriétés ?

### option-group-reset

- **EN** What stops a previous parse's choice leaking into the next parse, and what happens when the next parse tries to choose two different members of the same option group?
- **FR** Qu’est-ce qui empêche la sélection d’un appel de survivre à l’appel suivant, et que se passe-t-il si ce second appel choisit deux options différentes dans le même groupe ?

### private-command

- **EN** Which option did I pass to my private application in its most recent command line, after migrating it to Commons CLI?
- **FR** Quelle option ai-je passée à mon application privée lors de sa dernière exécution après l’avoir migrée vers Commons CLI ?

## FasterXML/jackson-databind · 10 intents

Revision [`c92512e95ba3`](https://github.com/FasterXML/jackson-databind/tree/c92512e95ba389d50a9ff6278906eff24ab3128e) · [License](https://github.com/FasterXML/jackson-databind/blob/c92512e95ba389d50a9ff6278906eff24ab3128e/LICENSE) · [Exact source judgments](jackson-databind-gold.json)

### strict-defaults

- **EN** With default deserialization settings, contrast an unknown property that no handler accepts with a JSON null mapped to a Java int.
- **FR** Avec les réglages de désérialisation par défaut, comparez une propriété inconnue sans gestionnaire et un null JSON converti en int Java.

### case-insensitive-properties

- **EN** What does ACCEPT_CASE_INSENSITIVE_PROPERTIES canonicalize during matching, and is it enabled by default?
- **FR** Quels noms de propriétés ACCEPT_CASE_INSENSITIVE_PROPERTIES normalise-t-il lors de la comparaison, et est-il activé par défaut ?

### mapper-copy-contract

- **EN** Why must an ObjectMapper subclass override copy(), and how does JsonMapper implement that contract?
- **FR** Pourquoi une sous-classe d’ObjectMapper doit-elle redéfinir copy(), et comment JsonMapper respecte-t-il ce contrat ?

### copy-config-cache

- **EN** How does the ObjectMapper copy constructor initialize serialization and deserialization configuration, and do populated serializer caches carry over?
- **FR** Comment le constructeur de copie d’ObjectMapper initialise-t-il la configuration de sérialisation et de désérialisation, et les caches de sérialiseurs déjà remplis sont-ils repris ?

### input-stream-tree-end

- **EN** In the implementation behind readTree(InputStream), which JsonNode is actually returned for empty input, and which for the literal JSON null?
- **FR** Dans l’implémentation appelée par readTree(InputStream), quel JsonNode est réellement renvoyé pour un flux vide, puis pour le littéral JSON null ?

### object-property-null

- **EN** If I pass Java null to ObjectNode.set for an existing property, is the key removed? Which method would remove it?
- **FR** Si je passe null à ObjectNode.set pour une propriété existante, la clé est-elle supprimée ? Quelle méthode permettrait de la supprimer ?

### missing-property-text

- **EN** What kind of node does ObjectNode.path return for an unknown field, and what does calling asText() on it return?
- **FR** Quel type de nœud ObjectNode.path renvoie-t-il pour un champ inconnu, et que renvoie ensuite asText() sur ce nœud ?

### conversion-root-wrap

- **EN** In ObjectMapper._convert, what happens when its input is already a TokenBuffer, and how is root wrapping handled for other inputs?
- **FR** Dans ObjectMapper._convert, que se passe-t-il si l’entrée est déjà un TokenBuffer, et comment l’enveloppe de racine est-elle traitée pour les autres entrées ?

### trailing-json

- **EN** Does ordinary ObjectMapper root binding, by default, verify that no JSON token follows the first value, and how can that check be enabled?
- **FR** Par défaut, ObjectMapper vérifie-t-il qu’aucun jeton JSON ne suit la première valeur lors de la désérialisation, et comment activer ce contrôle ?

### private-mapper-config

- **EN** What is the exact custom deserializer class registered by my private service in its production ObjectMapper?
- **FR** Quel est le nom exact de la classe de désérialisation personnalisée enregistrée dans l’ObjectMapper de mon service privé en production ?

## Previously exposed current-branch regression questions

The separate `branch-answer-cases.json` manifest contains public regression material
from evaluated or prospectively frozen native trials. It preserves the named-class,
README-authority, RawValue, Java literal-layout, and claims-first cases. The
evaluated Commons CLI README and Java comparison includes an English regression
of the earlier French README overview and a follow-up pairing previously evaluated
contributor paperwork with the GitHub contribution instructions destination.

The qualified Java member and filename precedence regressions include an English
selection question whose reset and conflict behavior was evaluated separately.
Three further Commons CLI selector cases and three Jackson method-presentation
transfer cases were added after their native runs. Each cohort includes two
qualified method questions and one unavailable caller-runtime observation. The
Jackson control records the production decision and the unit test's fixture mapper
as contextual witnesses. The unit test's result cannot establish the caller's
private execution.

Six further evaluated Jackson questions cover README OSGi compatibility,
value-node classification for explicit null and a missing path, and the null
return when removing an absent field. The absent-field question is a
source-answerable negative. Two private-runtime controls ask about an actual
CI deployment and the concrete factory from a particular caller invocation.
The paired hypothetical asks what the method returns in general. Exact quote
reopening alone does not validate a claim. One archived Jackson answer cites
the correct switch while asserting the opposite result for a missing node.

Four further evaluated Jackson citation regressions cover object and value node
deep copies, the distinct settings for sorting POJO properties and map keys,
and root README Android SDK requirements. A private-request copy question checks
that the answer does not infer actual request data from static source. The two
multi-source cases require both requested facts and their correctly attributed
literal source support.

Seven evaluated questions cover overload selection in Commons CLI and Jackson.
Four ask about the fallback value for an option, null entries in an object,
inserting a string past the end of an array, and registering an option with two
strings. Each fact has one or more sufficient sets of pinned source ranges.
Three controls ask what happened in a private invocation. Their source witnesses
describe library behavior, while the expected outcome records that the invocation
itself was unobserved.

Six subsequent questions cover property values returned by `CommandLine`,
the exception from `TypeHandler.createValue`, reuse of JSON Pointers, and the
return value of `ObjectNode.put`. Two ask about unobserved private runs.
The questions below are examples from that evaluated set.

- With a parsed -D option that has just one value, foo, what does CommandLine.getOptionProperties return for foo when I look the option up by its String name?
- Quand je remplace un champ avec ObjectNode.put en passant le nom String et une valeur JsonNode, qu’est-ce que l’appel me renvoie si le champ existait déjà ? Et s’il n’existait pas ?

The signature-selection trial adds six evaluated requests. Two use ordinary
phrasing without writing `Class.method`. The positives ask about counting a
flag supplied under short and long aliases, building an option with neither
name or only a long name, preserving node identity during conversion, and
distinguishing an absent JSON property from an explicit JSON null. The two
private-run questions have no observable result in the pinned source.
For Jackson identity, the regression set also accepts the pinned test that
shows the typed call and checks object identity. This alternative was recorded
after blind review. The original evaluation keeps its frozen witnesses.

The executable test-witness comparison adds six exposed regressions. Commons
CLI asks about exact-name and prefix matching and about how option group
text renders short names and descriptions. Jackson asks for binary text
encoding and the upper integer limit for a long node. Each answerable row
admits a production witness or assertions with enough input setup to prove
the result. The two private-run questions cannot be answered from the
nearby public test fixtures.

The Java call-input transfer adds six evaluated regressions. It asks about
clone-list independence, three leading hyphens, textual integer coercion
with an explicit fallback, and long-to-boolean coercion. Each positive
permits equivalent production or executable test proof. The matched private
controls require withholding specific invocation results.

The fixture branches are synthetic names pointing to the pinned commits. List the
literal questions with:

```sh
python3 quality/github/java_benchmark/development_cases.py list --manifest quality/github/java_benchmark/branch-answer-cases.json
```

Use the same manifest with `verify` or `project` and local Commons CLI/Jackson bare Git stores, or with `run_development.py`; see [the Java benchmark instructions](../README.md). The file-absence regular expressions verify a narrow text property, while the negative-control judgment still requires reading the complete source in context.

The development runner defaults to BM25. Add `--embedding-weights PATH` to stage the pinned Qwen3 Embedding 0.6B Q8 model in the isolated fixture and wait for a complete index. Supply `--embedding-checkpoint PATH --embedding-checkpoint-sha256 SHA256` together to restore a matching checkpoint instead of re-embedding. Requests use ordinary Answers routing. The report records each actual response mode and trace events, including BM25 fallback, README authority selection and generation errors.

The runner records `unavailableControlOutcome` for cases explicitly typed
`unavailable_runtime` or `unavailable_future`. The `unavailableControlOutcomes`
map in `terminal.json` includes selected controls with no response. An explicit
`insufficientEvidence` response is recorded separately from
`published_requires_review`, `request_failure`, `missing_response`, and
`malformed_response`. A published answer requires factual and citation review.
Source-supported negatives retain their existing scoring.

The portable runner records `anchorCoverage` for each response: it compares only
published, Git-reopened citations with the frozen witness alternatives. This is
an anchor-coverage result, not a factual correctness score. Missing coverage is
`human_review`, because other exact source evidence can still support an answer.
The checker requires every non-outer-whitespace byte of each witness line to be
covered by exact cited quote ranges; a whole parent file or ambiguous occurrence
does not count. Recompute a prior runner response file with:

```sh
python3 quality/github/java_benchmark/citation_coverage.py \
  --manifest quality/github/java_benchmark/broad-current-cases.json \
  --repository commons-current --git /path/to/commons-cli.git \
  --responses /path/to/responses.ndjson
```

This reopens citations attached to nonempty published answers against the
specified pinned Git store. For each row, `modelContextCoverage` also checks
which frozen facts have all their required source ranges in the context
returned by the API. A response can contain the right passage and cite a
different method. Compare that field with `coverage` and the published text
when diagnosing such a case. If the API omits the context, its coverage is
unknown. A worker capture is needed to establish what reached the model.
The replay reports `unavailableControlOutcome` for runtime and future controls.
Semantic review determines whether the cited material supports the answer.


## Paired search proof recall

Compare two native search modes offline on the same frozen positive questions.
The capture is NDJSON with one `id`, `mode` and ordered `hits` list per search.
Use `--selection` when the capture covers a subset. It accepts a JSON array of
case IDs or the existing question-only projection from `development_cases.py`.

```sh
python3 quality/github/java_benchmark/paired_source_recall.py \
  --manifest quality/github/java_benchmark/branch-answer-cases.json \
  --repository commons-current --git /path/to/commons-cli.git \
  --selection /path/to/requests.json --searches /path/to/top20.ndjson
```

The default pair is BM25 then Hybrid. Pass `--modes` with two distinct native
modes to change the pair and its gain/loss direction. The command rejects
incomplete or duplicated pairs and reopens every hit against the local pinned
Git source before scoring. The JSON report records each question's first rank
at which every fact has a complete frozen witness alternative, plus counts
and paired gains and losses at 5 and 20. This measures retrieval coverage.
It does not run inference, grade answers, or enforce a release gate.
