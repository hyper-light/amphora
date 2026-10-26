# The planner: techniques at their sources

Source notes for Amphora's planner (CLAUDE.md §5), from kiln research
`01a16eab7f47731291bbc7e2140f5e8a` (plan "The planner, researched and audited", the person's ask of
2026-10-24). Six topics: cardinality estimation, join ordering, unnesting, cost models, robust and
adaptive execution, and plan caching. For each, this note says what the papers found, what DuckDB,
Umbra/CedarDB, PostgreSQL, SQL Server, CockroachDB, ClickHouse and Calcite do, and where Amphora
stands. The planner's decisions are in `docs/design/planner.md`. This note is what they cite.
Section 7 audits Amphora's planner against these sources, each gap shown on a release build.

Every claim ends with its evidence in parentheses: the source, where in it, the words quoted, then
the research finding (`f11` to `f43`) and the grade kiln gave it (A to F, from the sources' kinds,
how far the quotes held up where cited, how many independent sources agree, and what is against
it). A grade is kiln's judgment of the evidence, not of the technique. Kiln doesn't read a PDF's
words, so a finding resting on one paper's PDF alone (f38, f40) is graded C with its quotes
unchecked. Those quotes were taken from the papers' text, kept under `~/.cache/planner-research/`.
Where a finding has a
counterclaim, the counterclaim is given beside it. The engines' code was read at fixed revisions:
DuckDB main at 1641475e (2026-10-23), PostgreSQL master at 126976a8 (2026-10-23), CockroachDB
master at d30c905f (2026-10-24), and ClickHouse master at eddf9eec (2026-10-23). Amphora's own
findings (f1 to f10, f36, f37) cite its tree as of 2026-10-24.

## Where Amphora's planner stands

- Statistics are SQLite's `sqlite_stat1` only: a table's row count, and per index the average
  rows per distinct value of each leftmost prefix. Without `ANALYZE` it assumes a million rows a
  table and fixed selectivities (crates/engine/stats.rs line 18: "Average number of rows per
  distinct key prefix, for each leftmost prefix"; cost_params.rs lines 106-118:
  "rows_per_table_fallback: 1_000_000.0,"; f2, A).
- A table's filters multiply (the independence assumption). The one exception is a lower and an
  upper bound on one column, which count as a closed range of 0.005 (cost.rs line 429:
  "fraction *= constraint.selectivity;"; f3, A).
- Join estimates were made independent of join order in the working tree this week (join.rs line
  130: "a join's estimate does not depend on the order its tables are read"; f4, A). CERT is not
  yet clean: campaign 2 ran 21,848 checks, 37 failed and 2,049 were skipped for want of an
  estimate; 35 were cause AF (fixed) and 2 remain open as cause AG (bench/oracles/findings.md line
  67: "AG. CERT, open: a FULL JOIN's estimate below its inner join's; an index seek's estimate
  against two filters'"; f5, B). Without statistics, an estimate can jump by up to five orders of
  magnitude when a query is made more restrictive: a cross join with one added conjunct went from
  1,000,000 to 160,000,000,000 rows (cert-50d4acc3fb3a85cd.md line 1: "the more restrictive query
  is estimated at 160000000000.00003 rows, the query at 1000000"; f6, B).
- Join order: left-deep only, by a dynamic program over table subsets each extended by one table up
  to 12 tables, then a greedy left-deep search (join.rs line 1576: "In this block, LHS is always
  (n-1) tables, and RHS is a single table."; line 1813: "pub const GREEDY_JOIN_THRESHOLD: usize =
  12;"; f1, A). A RIGHT or FULL JOIN is a reordering barrier in both directions (join.rs line 2000;
  f9, A).
- Unnesting is by fixed patterns: EXISTS and NOT EXISTS become semi- and anti-joins, a positive IN a
  semi-join, and a one-aggregate subquery is grouped by its correlation columns. NOT IN stays a
  subquery, and nothing is unnested beside a RIGHT or FULL JOIN (unnest.rs line 106: "`NOT IN` stays
  as a subquery because NULL values can change its result."; f7, A).
- Cost is one abstract unit over page reads, seeks, row, filter, hash and sort work, with Mackert
  and Lohman's finite-LRU formula for table lookups (OPTIMIZER.md line 62; f9, A).
- A statement is planned at prepare and again only when re-prepared after a schema change. There is
  no shared plan cache, and a parameter's value never causes a new plan (f8, B).
- Per-operator estimation error can't be measured yet: metrics are whole-statement totals and
  `EXPLAIN QUERY PLAN` carries estimates but no actual counts (vdbe/metrics.rs line 72: "pub
  rows_read: u64,"; f36, B). The workloads to judge the planner are at hand: JOB's 113 queries are
  cloned locally, and the G2 corpus goal names TPC-H, TPC-DS, JOB, ClickBench, SSB and CH-benCHmark
  (f37, A).
- The rules bound how any change is built: no recursion, no depth-first walk of user-scaled
  structures on the call stack, and every loop over user-scaled data ends on a counted budget
  (CLAUDE.md line 86: "No function calls itself, directly or through others, and no algorithm walks
  a user-scaled structure"; f10, A). This matters below: DPhyp as published, DuckDB's enumerator and
  both Neumann unnesting papers are stated recursively.

## 1. Cardinality estimation

### What the sources found

**Estimates are the main cause of bad plans.** On the Join Order Benchmark (113 multi-join queries
over IMDb), every estimator tested routinely erred by a factor of 1,000 or more. The errors grew
exponentially with the number of joins, and multi-join results were systematically underestimated,
which the authors trace to the independence assumption PostgreSQL makes (Leis et al., PVLDB 9(3)
2015, line 82: "113 multi-join queries"; section 3.1 line 575: "we routinely observe misestimates
by a factor of 1000 or"; line 577: "the errors grow exponentially (note the logarithmic scale)";
line 625: "tend to systematically underestimate"; section 2.3 line 306: "PostgreSQL simply assumes
independence and multiplies the selectivities"; LpBound's abstract: "is often the main culprit when
the optimizer chooses a poor plan"; f11, A).

**Sampling.** Systems that keep table samples estimated single-table results considerably better on
JOB. The authors urge "table samples as a simple and robust technique", but single-table samples
miss join-crossing correlations (Leis et al. 2015, section 7 line 1500: "systems should be adopting
table samples as a simple and robust technique"; line 1537: "many of our JOB queries contain
join-crossing correlations, which single-table samples do not capture"). Index-based join sampling,
run under a budget of index lookups, gave "much better plans than traditional estimators" (Leis,
Radke et al., CIDR 2017, section 3 line 382: "during sampling and stops once the given budget (e.g.,
100,000 index lookups) is exhausted."). Oracle samples table blocks at parse time when statistics
are missing, stale or blind to correlation (Oracle 19c SQL Tuning Guide, Supplemental Dynamic
Statistics: "The database uses recursive SQL to scan a small random sample of table blocks."; f13,
D). Against it: on STATS-CEB, Han et al. found uniform and wander-join sampling over joins worse end
to end than PostgreSQL's own estimator. Neither is index-based join sampling or a per-table sample
used for filters (Han et al., PVLDB 15(4) 2022, section 5.1, O1: "Traditional histogram and
sampling based methods"; strong). Kiln grades f13 D for that counterclaim and for quotes it could
not check.

Umbra keeps its samples fresh online. Inserts maintain the sample as they happen, with "virtually
zero overhead in insert performance" and none of the random I/O a rebuilt sample costs; the price
is relaxed freshness across threads. A deleted sampled tuple is replaced by an upcoming one at the
same position (Birler, Radke, Neumann, DaMoN 2020, abstract: "It is much more attractive to use
online sampling, such that a fresh sample is available at all times, without additional data
accesses."; abstract: "We can trade off strict freshness guarantees for a significant boost in
performance"; section Updates and Deletes: "our online approach needs to pick an upcoming tuple to
replace the deleted tuple."; Umbra, CIDR 2020, section 3 line 568: "a scalable online reservoir
sampling algorithm that we recently developed [2]"; f41, A).

**Sketches.** HyperLogLog estimates cardinalities beyond 10^9 "with a typical accuracy of 2% while
using a memory of only 1.5 kilobytes." (Flajolet et al., AofA 2007, abstract line 22). Umbra keeps
updatable HyperLogLog sketches on every column beside its sample. Together they give almost perfect
per-column distinct counts and, with the sample, accurate multi-column estimates (Neumann and
Freitag, CIDR 2020, section 3 line 525: "Umbra tracks a number of statistics for query optimization
purposes. First of all, a random sample of each relation is maintained."; line 572: "shown
previously [6], our implementation of updateable HyperLogLog sketches can provide almost perfect
cardinality estimates"; Freitag and Neumann, CIDR 2019, abstract: "with random sampling to correct
for correlation bias between"; f14, A).

**Correlated columns.** PostgreSQL's docs say "It is common to see slow queries running bad
execution plans because multiple columns used in the query clauses are correlated." Their extended
statistics must be created by hand because "it's impractical to compute multivariate statistics
automatically." (PostgreSQL docs 14.2.2; clausesel.c line 70: "probabilities, and in reality they
are often NOT independent even if they"; f17, A). SQL Server's CE 120 and later assumes partial
correlation, combining selectivities by "a variation on exponential backoff, ordering the
selectivities from most to the least selective predicate." CE feedback adjusts that per query and
keeps "Only verified feedback" (Microsoft Learn, CE feedback; f17, A).

**Learned estimators.** On static data they are more accurate than traditional ones, but they cost
much to train and run, can't keep up with fast updates, and are "much harder to interpret and
often unpredictable." A simple adaptive-execution approach matched or beat reinforcement-learned
optimizers with no training (Wang et al., PVLDB 14(9) 2021, abstract: "learned models are indeed
more accurate than traditional methods"; Zhang et al., PVLDB 16(11) 2023, abstract line 41:
"competitive to the SOTA RL-based approaches but, in some cases,"; f18, D). Against it: Han et
al.'s end-to-end benchmark found ML data-driven estimators (BayesCard, DeepDB, FLAT) markedly
improving PostgreSQL's query times, where most traditional methods "hardly improve the PostgreSQL
baseline." (section 1, K1; strong). Kiln grades f18 D for that.

**Pessimistic bounds.** LpBound computes "a guaranteed upper bound on" multi-join outputs from
lp-norms of degree sequences. It was orders of magnitude more accurate than mainstream estimators
on JOB and STATS, and with its bounds PostgreSQL chose plans "at least as good as those derived
using" true cardinalities (Zhang et al., arXiv:2502.05912, abstract). In Han et al.'s independent
benchmark, the pessimistic estimator PessEst also did significantly better than PostgreSQL (section
5.1, O1: "whereas the new traditional approach"; f19, A). SafeBound made bounds practical with
predicates and compressed degree sequences: "By never underestimating, this approach allows the
optimizer to avoid potentially inefficient plans." It "achieves up to 80% lower end-to-end runtimes
than PostgreSQL" and "saves up to 500x in query planning time". Its authors say "the current
SafeBound prototype is likely not yet ready for a production system": "Any upper bound system must,
at some point, read all rows in the database."; "A challenge that we leave open is handling updates
without recomputing the degree sequences."; and several predicates on one table give "an estimated
cardinality equal to the selectivity of the most selective predicate." (Deeds, Suciu, Balazinska,
SIGMOD 2023, abstract and section Limitations and Future Work; f40, C).

**Measuring error: q-error and P-error.** The q-error is the factor between an estimate and the
truth, and it bounds a plan's cost at "a factor of q 4" times the optimum (Moerkotte, Neumann,
Steidl, PVLDB 2(1) 2009, abstract). But it "cannot distinguish the importance of" sub-plans and
doesn't track end-to-end time. Han et al.'s P-error "is highly correlated with the end-to-end
query time improvement." (Han et al. 2022, abstract and section 1; f20, A).

**CERT's evidence.** CERT (cardinality restriction monotonicity) checks that a more restrictive
query, such as a LEFT JOIN made INNER or a conjunct added, is never estimated to return more rows:
its estimate "should not exceed the estimated number of rows for the original". It targets
estimation because estimation "was shown to be the most important part for query optimization". It
found 13 unique issues in MySQL, TiDB and CockroachDB (Ba and Rigger, ICSE 2024, abstract: "We
evaluated CERT on three widely used and mature DBMSs, MySQL, TiDB, and CockroachDB."; "CERT found 13
unique issues, of which 2 issues were fixed and"; f21, A). Amphora's oracles run it (f5, f6 above).

### What the engines do

| engine | estimation | evidence |
|---|---|---|
| DuckDB | A join's cardinality is a property of its relation set, cached per set: base cardinalities over a denominator built from the join predicates' equivalence-class domains. Distinct counts come from HyperLogLog over a 10% sample of each vector (30% for integers). | cardinality_estimator.cpp line 188: "reference_map_t<JoinRelationSet, CardinalityHelper> relation_set_2_cardinality;"; line 902: "The estimator starts with base relation cardinalities and divides by a denominator assembled from predicate domain"; distinct_statistics.hpp lines 52-54: "static constexpr double BASE_SAMPLE_RATE = 0.1;" (f15, A) |
| DuckDB (its source) | Ebergen's thesis assumes every join is a foreign-key/primary-key join (containment of value sets). Transitively joined columns form equivalence sets sharing one total domain: the largest known distinct count, or else the lowest-cardinality column taken as the primary key. Each dynamic-programming entry's cardinality is a property of its relation set. Without statistics, every filter gets a flat 20%. On JOB: 66% fewer intermediate tuples on Parquet, 95% on base tables, and 18% and 25% faster end to end. | thesis abstract: "By assuming all joins are foreign-key primary-key joins"; "join order optimizer improved end-to-end execution times by 18% on average"; section 3.1: "Every entry in the dynamic programming table is represented as a set of relations, and now the cardinality is also represented as a set of relations."; section 3.2: "then the column with the lowest cardinality in the equivalence set is assumed to be a primary key"; section 3.4: "selectivity of 20%. In the presence of HyperLogLog statistics"; DuckDB cites it at cardinality_estimator.cpp line 900 (f39, A) |
| Umbra / CedarDB | A random sample of every relation, kept by online reservoir sampling, plus updatable HyperLogLog on every column. CedarDB says its statistics estimate group-by result sizes. | Umbra CIDR 2020 section 3 lines 525, 568, 572 (above); CedarDB docs, State-of-the-art query optimizer: "estimate relation size after group by operations" (f14, A; f41, A) |
| PostgreSQL | One rowcount for every path of a join relation. Extended statistics (functional dependencies, multi-column distinct counts, MCV lists) are applied first, by hand; the remaining clauses multiply. | costsize.c line 5679: "anyway we must keep the rowcount estimate the same for all paths for the"; clausesel.c line 68: "The remaining clauses are then estimated by taking the product of their" (f16, A; f17, A) |
| SQL Server | Partial correlation by exponential backoff (CE 120 and later), CE feedback per query, and a containment assumption for joins. CE 70's simple containment applies each input's filter selectivity to its histogram before joining histograms. CE 120's base containment estimates the join from the base tables' histograms, then applies the filters, and assumes filters on different tables are uncorrelated. A hint restores simple containment. | Microsoft Learn, Cardinality Estimation, Versions of the CE: "Users query for data that exists. For example, for an equality join between two tables, factor in the predicates selectivity"; "Users might query for data that doesn't exist. For example, for an equality join between two tables, we use the base tables histograms to estimate the join selectivity, and then factor in the predicates selectivity."; Example C: "predicate filters from different tables usually don't correlate with each other."; Query Hints, ASSUME_JOIN_PREDICATE_DEPENDS_ON_FILTERS: "Generates a query plan using the Simple Containment assumption instead of the default Base Containment assumption for joins"; Versions of the CE: "The combination of the different column values isn't necessarily independent." (f43, A; f17, A) |
| CockroachDB | Every expression in a memo group shares the group's logical properties, row count among them. CERT found issues in its estimator. | memo.go line 62: "expressions in a group share the same logical properties" (f16, A); CERT abstract (f21, A) |
| ClickHouse | Column statistics, GA since 26.3 and used by default, built automatically on every suitable column (basic and uniq_v2 by default since 26.7) and materialized on insert. t-digest answers less-than predicates, count-min sketches equalities (0.1% error at 99.9% confidence), uniq (HyperLogLog) distinct counts. Columns multiply (independence). Without statistics, fixed factors: 0.33 a range, 0.01 an equality. | Settings.cpp line 10803: "Column statistics are now GA."; line 2416: "so cost-based join reordering has good estimates on freshly-loaded dimension tables."; MergeTreeSettings.cpp lines 2465-2467: "Supported statistics types: basic, tdigest, countmin, uniq, uniq_v2."; Statistics.cpp line 340: "TDigest: '< X' (less-than predicates)"; StatisticsCountMinSketch.cpp lines 21-22: "an error tolerance of 0.1% (ε = 0.001)"; ConditionSelectivityEstimator.cpp line 1016: "The map enforces independence across columns"; ConditionSelectivityEstimator.h line 164: "static constexpr Float64 default_cond_range_factor = 0.33;" (f42, A) |
| Calcite | Estimation is not in the research; its cost combines CPU, I/O and memory (section 4). | (f29, A) |

The three planners that make a join's estimate a property of the set of relations joined, not of
the path or order that builds it, are PostgreSQL, CockroachDB and DuckDB. That makes an estimate
order-independent by construction (f16, A). Amphora's AF fix gets the same property differently:
a constraint is ready once all its tables are joined (f4).

## 2. Join ordering

### What the sources found

**Exact enumeration: DPsize, DPccp, DPhyp.** DPsize, which Amphora's subset-by-size search
resembles, runs far above the lower bound. DPccp meets it, and DPhyp extends DPccp to hyperedge
predicates and non-inner joins, improving optimization time "by orders of magnitude, compared to
known algorithms dealing with complex join predicates and non-inner joins." As published, DPhyp's
subgraphs and complements "are created by recursive graph traversals" (Moerkotte and Neumann,
SIGMOD 2008, section 1 line 26: "We proved that DPsize has a runtime complexity"; line 65: "DPhyp,
which is capable to handle complex join predicates"; section 3 line 295; f22, A).

**Conflict detection for outer, semi and anti joins.** ClickHouse and CockroachDB reorder non-inner
joins exactly with the CD-C conflict detector of Moerkotte, Fender and Eich, which is "correct and
complete". ClickHouse runs it in DPsub; CockroachDB runs it in DPSube (ClickHouse Settings.cpp line
11355: "the CD-C conflict detector, which is correct and complete"; CockroachDB
join_order_builder.go line 137: "The paper 'On the correct and complete enumeration of the core
search space'"; f25, A). Amphora treats RIGHT and FULL JOIN as barriers instead (f9).

**Heuristics leave performance on the table.** On JOB, exhaustive dynamic programming beat
heuristics even with estimation errors: with true cardinalities and PK+FK indexes, GOO's plans were
up to 21x costlier and Quickpick's up to 32x. Left-deep trees cost at most 4.5x the best bushy plan,
and zig-zag trees at most 2.54x (Leis et al. 2015, section 1 line 142: "using heuristics leaves
performance on the table"; section 6.2 line 1491: "being 2.54× more expensive than the best bushy
plan"; f27, A). Greedy Operator Ordering as published keeps a set of trees and merges the pair with
the lowest cost, so it builds bushy trees. The Amphora search labelled GOO instead adds one table at
a time to a left-deep tree (Leis et al. 2015 lines 1452-1453: "The algorithm then combines the pair
of join trees with the lowest"; Neumann and Radke 2018 line 273: "the best known greedy heuristics
is Greedy Operator Ordering [8],"; Amphora join.rs line 1817: "Builds a left-deep join tree by:";
f28, A).

**Adaptive and greedy hybrids for large queries.** Neumann and Radke adapt to the query graph
rather than switching to greedy at a fixed table count. They run DPhyp outright below 14 relations.
Up to 100 relations, they "count the number of connected subgraphs up to the budget of 10,000" and
solve exactly within it. Beyond that they use search-space linearization, near-optimal up to 100
relations, and add a greedy step only for larger queries. Real queries reach "more than 4,000
relations" (SIGMOD 2018, lines 377-380, 325: "But instead of switching to a greedy approach, as some
other systems do, it uses a novel search space linearization technique to"; line 45; f23, A).

### What the engines do

| engine | join ordering | evidence |
|---|---|---|
| DuckDB | DPhyp; exact search stops after 10,000 emitted pairs, then Greedy Operator Ordering; approximate from 12 relations (`approximate_join_order_threshold`). The connected-subgraph enumeration is a recursive function. | plan_enumerator.cpp line 580: "the plan enumeration is a straight implementation of the paper "Dynamic Programming Strikes Back" by Guido"; lines 243-244: "when the amount of pairs gets too large we exit the dynamic programming and resort to a greedy algorithm"; line 410; line 348: "bool PlanEnumerator::EnumerateCSGRecursive(JoinRelationSet &node, unordered_set<RelationIndex> &exclusion_set) {"; settings.hpp lines 312-316: "The minimum number of tables in a join to determine the optimal join order approximately instead of exactly." (f24, A; f22, A) |
| Umbra / CedarDB | Neumann and Radke's adaptive scheme (above). CedarDB says its optimizer is "extensively tested for extremely large queries" (more than 10,000 joins). | CedarDB docs, State-of-the-art query optimizer: "full-fledged cost-based query optimizer extensively tested for extremely large queries" (f23, A) |
| PostgreSQL | Bushy trees without cross products, searched exhaustively up to the collapse limits (8 FROM items by default); GEQO from 12 FROM items. | PostgreSQL docs 19.7, geqo_threshold: "Use genetic query optimization to plan queries with at least this many"; "The default is 12."; from_collapse_limit: "The default is eight."; Leis et al. 2015 line 298: "Join orders, including bushy trees but excluding trees with" (f26, A) |
| SQL Server | Not in the research. Its join decisions under uncertainty are in section 5. | — |
| CockroachDB | DPSube with CD-C; reorders at most 8 joins by default. | join_order_builder.go line 123: "Then, the DPSube algorithm is executed (see citations: [8]). DPSube"; docs, Join reordering: "only used to reorder subtrees containing 8 or fewer joins by default" (f25, A) |
| ClickHouse | Default `dpsub,greedy` since 26.11: DPsub with CD-C up to 12 relations, then greedy, under a deterministic budget of 100,000 partial plans. DPhyp offered for inner joins only. | Settings.cpp line 11340: "DECLARE(JoinOrderAlgorithm, query_plan_optimize_join_order_algorithm, "dpsub,greedy", R"("; line 11344: "It only applies to queries with at most 12 relations; above that it gives way to the next algorithm in the list."; line 11345: "implements DPhyp (Dynamic Programming via Hypergraph Partitioning) algorithm currently only for inner joins"; line 3130: "Maximum number of partial plans the join order optimizer may enumerate before giving up and falling back to the next algorithm"; line 11342: "basic greedy algorithm - works fast but might not produce the best join order" (f25, A; f22, A; f27, A) |
| Calcite | A Volcano-style cost-based dynamic program over rule-generated alternatives, and a heuristic Hep planner firing rules to a fixed point. Large joins are ordered by the heuristic LoptOptimizeJoinRule. | Begoli et al., SIGMOD 2018, section 6: "uses a dynamic programming algorithm, similar to Volcano"; "which triggers rules exhaustively until it generates an expression that is no longer modified by any rules"; LoptOptimizeJoinRule javadoc: "implements the heuristic planner for determining optimal join orderings" (f29, A) |

## 3. Unnesting

### What the sources found

**Dependent joins (2015).** Correlated subqueries "frequently lead to dependent joins with nested
loops evaluations and thus poor performance". Neumann and Kemper turn every correlated subquery
into a dependent join and push it down until no correlation is left, for arbitrary queries. Earlier
systems used heuristics limited to classes of queries (BTW 2015, abstract line 9; line 13: "no
existing system can de-correlate queries in the general case. We present a generic"; f30, B).

**Top-down unnesting (2025).** When correlated subqueries nest, the 2015 method re-unnests
fragments. Neumann's single-pass top-down variant fixes this, visiting each operator at most once
(arXiv:2412.04294 abstract: "Recent work has introduced a single pass top-down algorithm for
unnesting arbitrary SQL queries."; section 5 line 952: "The unnesting algorithm visits each operator
at most once, starting from dependent joins in top-to-bottom order and stopping when the current
tree no longer"; f30, B). The BTW 2025 paper behind it says existing techniques "are either limited
in scope or lead to suboptimal execution plans when correlated queries are stacked repeatedly inside
each other". The 2015 algorithm is used by HyPer, DuckDB and Umbra, and "The original algorithm
does that bottom-up." The new one "can be seen as a top-down variant of [NK15]" and adds "recursive
SQL and ORDER BY LIMIT constructs." On the paper's Figure 1 query, PostgreSQL 16 needs over 26
minutes on TPC-H SF1; "After applying our algorithm, Postgres can execute the query in 1.3s, a
speed-up of more than 1000x." Umbra with the old logic exhausts 120 GB, while "Using the new
top-down formulation it finishes in 33ms on the same machine." Its per-operator rules are stated as
recursion ("and recurse to the input"), so Amphora would need them as a loop over an explicit
worklist (f10) (Neumann, BTW 2025, abstract p. 25, section 1 pp. 26-27, section 3 p. 31, section
3.3 p. 36, section 5 p. 44; f38, C).

### What the engines do

| engine | unnesting | evidence |
|---|---|---|
| DuckDB | Decorrelates every subquery with the 2015 algorithm. | DuckDB blog 2023-05-26: "DuckDB uses a state-of-the-art subquery decorrelation algorithm as described in the" (f31, A; f30, B); BTW 2025 p. 26: "Both Hyper and DuckDB use it in commercial offerings, our research system Umbra uses it" (f38, C) |
| Umbra / CedarDB | Umbra has integrated the 2025 top-down method. CedarDB fully decorrelates complex dependent subqueries, quadratic to linear. | BTW 2025 section 7: "The new approach has already been integrated into our Umbra system and we hope that other systems like DuckDB and Postgres will follow." (f38, C); CedarDB docs: "fully decorrelate very complex dependent subqueries"; "reducing them from quadratic runtime to linear runtime" (f31, A) |
| PostgreSQL | Doesn't unnest the Figure 1 query: over 26 minutes on TPC-H SF1, 1.3 s once the algorithm is applied by hand. | BTW 2025 section 5, p. 44 (f38, C) |
| SQL Server | Not in the research. | — |
| CockroachDB | Not in the research. | — |
| ClickHouse | Not in the research. | — |
| Calcite | Not in the research; its rules are applied by the two planner engines (section 2). | (f29, A) |

Amphora unnests by fixed patterns and cites both Neumann papers as references (f7).

## 4. Cost models

**Estimates matter more than costs.** On JOB, cost-model errors were "dwarfed by cardinality
estimation errors" and simple cost models sufficed. DuckDB's join-order cost is essentially each
join's estimated cardinality plus its inputs' costs (Leis et al. 2015, section 1 line 138: "cost
model errors is dwarfed by cardinality estimation errors"; abstract line 44: "we find that it has
much less influence on query performance than"; DuckDB cost_model.cpp line 37: "Currently cost of
a join mostly factors in the cardinalities."; line 47: "return join_cost + left.cost +
right.cost;"; f12, D). Against it: Amphora's own bench found plans doing far more work from
cost-model gaps, not estimates: a sort charged to the wrong candidate, and table lookups charged too
few pages. Those were fixed in planner.md decisions 2, 5 and 7 (docs/design/planner.md, decision 5:
"Mackert and Lohman count about 188,000 pages read for 200,000"; strong). Kiln grades f12 D for
that. For Amphora's point and range statements, the cost model mattered.

| engine | cost model | evidence |
|---|---|---|
| DuckDB | Join cost is the cardinality of each join plus its inputs' costs. | cost_model.cpp lines 37, 47 (f12, D) |
| Umbra / CedarDB | Not in the research. | — |
| PostgreSQL | Not in the research beyond JOB's finding that its cost model mattered less than its estimates. | Leis et al. 2015 (f12, D) |
| SQL Server | Not in the research. | — |
| CockroachDB | Not in the research. | — |
| ClickHouse | Not in the research. Its join order runs under a budget of partial plans (section 2). | (f25, A) |
| Calcite | Default cost combines CPU, I/O and memory estimates. | Begoli et al. 2018, section 6: "CPU, IO, and memory resources used by a given expression." (f29, A) |

## 5. Robust and adaptive execution

**Guards on decisions that hinge on estimates.** SQL Server's batch-mode adaptive join defers the
choice between hash join and nested loops until its first input is scanned: it "defines a threshold
that is used to decide when to switch to a Nested Loops plan." and "Your plan can therefore
dynamically switch to a better join strategy during execution." (Microsoft Learn, Intelligent query
processing details). On JOB, nested-loop joins without an index were the risky choice: "this
decision is extremely risky". With them removed, "we observed no more timeouts" under PostgreSQL's
estimates (Leis et al. 2015, section 4.1 lines 894, 899; f32, A).

**Execution-time robustness.** Lookahead Information Passing pushes Bloom filters through left-deep
star-join pipelines (Zhu et al., PVLDB 10(8) 2017, section 1 line 25: "techniques that increase the
robustness of query plans, by"). Predicate transfer pre-filters every table through the join graph
with Bloom filters: 3.3x over Bloom join on TPC-H, and "regardless of the chosen join order, the
join phase" is robust (Yang et al., CIDR 2024, section 2 line 134). POLAR routes tuples among
alternative left-deep orders in DuckDB with bounded regret, "of up to 9x and less than 7% overhead
for all benchmark queries," (Justen et al., PVLDB 17(6) 2024, abstract line 70). Diamond-hardened
joins split joins into Lookup and Expand: up to 500x on the CE benchmark, with no loss on TPC-H or
JOB (Birler, Kemper, Neumann, PVLDB 17(11) 2024, abstract line 43: "to traditional joins, queries’
performance is improved by up to 500x"; f33, A).

**Re-optimization.** A simple mid-query re-optimization on observed cardinalities cut the
end-to-end latency of JOB's 20 longest-running queries by 27%, most of what perfect estimates would
give: re-optimization "is likely the most cost-effective way" (Perron et al., ICDE 2019, abstract:
"queries in the Join Order Benchmark by 27%, realizing most of"). Holistic adaptive processing is
still absent from commercial systems because it complicates debugging and testing (POLAR, abstract
line 60: "challenges. However, commercial database systems in practice do"; f34, A).

| engine | robust or adaptive execution | evidence |
|---|---|---|
| DuckDB | POLAR was built and measured in DuckDB (a research prototype, not DuckDB's release). | (f33, A) |
| Umbra / CedarDB | Diamond-hardened joins come from Umbra's group. The research does not say whether CedarDB ships them. | (f33, A) |
| PostgreSQL | No adaptive join in the research. JOB's timeouts under its estimates ended once unindexed nested loops were removed. | (f32, A) |
| SQL Server | Batch-mode adaptive joins: hash or nested loops chosen at run time against a threshold, within one cached plan. | (f32, A) |
| CockroachDB | Not in the research. | — |
| ClickHouse | Not in the research. | — |
| Calcite | Not in the research. | — |

## 6. Plan caching

Parameter-sensitive plans are handled three ways (f35, A):

- **Custom then generic.** PostgreSQL and CockroachDB plan a prepared statement with its values for
  the first five executions, then switch to a generic plan if its estimated cost is not
  significantly above those custom plans' average. PostgreSQL's PREPARE docs: "first five
  executions are done with custom plans and the average estimated cost of those plans is
  calculated."; 19.7, plan_cache_mode: "Custom plans are made afresh for each execution using its
  specific set of parameter values, while generic plans do not rely on the parameter values and
  can be re-used across executions."; CockroachDB docs, Query plan type: "Subsequent executions use
  a generic plan if its estimated cost is not significantly higher than the average cost of the
  preceding custom plans."
- **Several cached variants.** SQL Server's Parameter Sensitive Plan optimization keeps several
  plans for one parameterized query, dispatched by the parameters' estimated cardinality ranges:
  "It addresses the scenario where a single cached plan for a parameterized query isn't optimal for
  all possible incoming parameter values." (Microsoft Learn, PSP optimization, introduction).
- **Recompile on a new value.** SQLite recompiles a prepared statement when a newly bound value
  could change its plan: it "will be automatically recompiled, as if there had been a schema
  change" (sqlite3_prepare_v2, item 3).

| engine | plan caching | evidence |
|---|---|---|
| DuckDB | Not in the research. | — |
| Umbra / CedarDB | Not in the research. | — |
| PostgreSQL | Custom plans for five executions, then generic if not significantly costlier; `plan_cache_mode` overrides. | (f35, A) |
| SQL Server | PSP optimization: several plan variants per query. Batch-mode adaptive joins adapt inside one cached plan (section 5). | (f35, A; f32, A) |
| CockroachDB | Custom then generic, as PostgreSQL. | (f35, A) |
| ClickHouse | Not in the research. | — |
| Calcite | Not in the research. | — |

Amphora plans once at prepare and never re-plans for a parameter value (f8). It holds SQLite's
interface but not SQLite's recompile-on-bind rule.

## Engine by topic

Each engine's coverage across the six topics, by finding. A dash means the research holds nothing
for that engine on that topic.

| engine | estimation | join ordering | unnesting | cost models | robust/adaptive | plan caching |
|---|---|---|---|---|---|---|
| DuckDB | f15, f16, f39 | f22, f24, f28 | f30, f31, f38 | f12 | f33 | — |
| Umbra / CedarDB | f14, f41 | f23 | f31, f38 | — | f33 | — |
| PostgreSQL | f11, f16, f17 | f26 | f38 | f12 | f32 | f35 |
| SQL Server | f17, f43 | — | — | — | f32 | f35 |
| CockroachDB | f16, f21 | f25 | — | — | — | f35 |
| ClickHouse | f42 | f22, f25, f27 | — | — | — | — |
| Calcite | — | f29 | — | f29 | — | — |

The research's general findings, which name no single engine: f13 (sampling, with Oracle), f18
(learned estimators), f19 and f40 (pessimistic bounds), f20 (q-error and P-error), f21 (CERT), f34
(re-optimization).

## 7. The audit: Amphora's planner against the research

What Amphora's planner does today, row by row, against what the sources found and the leading
engines do, each gap shown on a release build. The code is cited at commit f734df4, whose engine
(`crates/`) is dbef789's unchanged; the demonstrations ran on adacachy on 2026-10-25, on
amphora-bench built in release (`bench/Cargo.toml`'s profile) from f734df4. Each demonstration is
a script in `bench/planner/audit/` (`NN-*.sql`) run by `amphora-bench --eqp` (`bench/src/eqp.rs`:
a new database each script, each statement's rows printed, `-- counted` to print each table
access's estimate beside the rows it counted, `-- keep` and `-- rerun` to run a statement prepared
earlier again), its output kept beside it (`NN-*.out`). `bench/planner/audit/run.sh BINARY
--check` reruns them all and the replay of the oracles' CERT findings and compares each with the
output kept (planning times left out); it passed on f734df4. Two outputs need what run.sh
doesn't hold and say how they were made in their scripts: `tpcds-subqueries.out`
(`tpcds_subqueries.sh`, reading TPC-DS's queries from turso's perf/tpc-ds at fbcfa43) and
`panics.out` (`panics.py`, over the count pass of scripts/check-production.sh). Without ANALYZE every table is
estimated at 1,000,000 rows, so the demonstrations without data show the fallback estimates.

Which benchmark queries a row touches comes from the queries' text, in
`bench/planner/audit/benchmarks.tsv` (`features.py`: each query's most tables in one FROM clause,
its join kinds and its subqueries, correlated or not): JOB's 113 queries (gregrahn/
join-order-benchmark at a396036), TPC-H's 22 (`bench/planner/tpch/queries`), TPC-DS's 99 (turso's
`perf/tpc-ds` at fbcfa43), CH-benCHmark's 22 (benchbase at 33c0047) and ClickBench's 43
(ClickBench's `sqlite/queries.sql` at 2e258dcb). SSB's 13 queries are read from ClickHouse's
listing of their joined form (clickhouse.com/docs/getting-started/example-datasets/star-schema,
fetched 2026-10-25: Q1.1 to Q1.3 "FROM lineorder, date"; Q2.x add part and supplier, Q3.x
customer and supplier, Q4.x all four), citing "Star Schema Benchmark (O'Neil et. al), 2009". In
short: JOB's queries join 4 to 17 tables (3 of 4, 20 of 5, 21 of 8, 11 of 12, 6 of 14 and 3 of 17),
inner joins only, no subqueries; TPC-H's join at most 8; TPC-DS's at most 18 (Q64), with LEFT
JOINs in 11 queries and FULL JOINs in Q51 and Q97; SSB's at most 5; CH-benCHmark's at most 9
(Q8); ClickBench's read one table.

| # | gap | Amphora's code (f734df4) | findings (kiln grade) | demonstration |
|---|---|---|---|---|
| 1 | left-deep trees only | join.rs 1598, 1617 | f1 A, f22 A, f26 A, f27 A | 01-left-deep-greedy |
| 2 | above 12 tables a greedy left-deep search, called GOO | join.rs 1432, 1854-1858, 1940, 1994 | f1 A, f23 A, f24 A, f27 A, f28 A | 01-left-deep-greedy |
| 3 | subsets enumerated by size, not as connected pairs | join.rs 1598-1599, 1634, 2523 | f1 A, f22 A, f23 A, f10 A | 02-subsets-by-size |
| 4 | no conflict detector: RIGHT and FULL JOIN barriers, outer, semi and anti joins held behind every table before them | join.rs 2026-2052 | f9 A, f25 A | 03-right-full-barrier |
| 5 | statistics only sqlite_stat1; 1,000,000 rows and fixed selectivities without ANALYZE; filters multiplied as independent | stats.rs 10-42; cost_params.rs 106-119; cost.rs 357, 408-458; constraints.rs 486-523 | f2 A, f3 A, f11 A, f13 D, f14 A, f15 A, f17 A, f39 A, f41 A, f42 A, f43 A | 04-statistics |
| 6 | a join's estimate from the access path chosen, not from the set of tables joined (AG) | join.rs 216-264; access_method.rs 1116, 1578, 1917; cost.rs 457 | f4 A, f5 B, f6 B, f15 A, f16 A, f21 A | 05-estimate-by-path, cert-replay |
| 7 | a FULL JOIN's estimate leaves out the right table's unmatched rows | join.rs 282-329; access_method.rs 1578-1584 | f5 B, f16 A, f21 A | 06-full-join-estimate |
| 8 | a join on columns no index leads takes a fixed 0.1, with no key or containment | constraints.rs 515-517; cost_params.rs 110 | f15 A, f39 A, f43 A, f11 A | 11-join-size |
| 9 | a GROUP BY's groups are not estimated | eqp.rs 950-960 | f14 A | 11-join-size |
| 10 | unnesting by fixed patterns | unnest.rs 1-7, 106, 154-158, 1267; mod.rs 999-1035 | f7 A, f30 B, f31 A, f38 C, f10 A | 07-unnesting, 13-tpch-subqueries, tpcds-subqueries |
| 11 | plans fixed at prepare: no re-plan for new statistics or a parameter's value | statement.rs 709-735, 751, 1040, 1453 | f8 B, f35 A | 08-fixed-at-prepare |
| 12 | a page-based cost model whose pages are rows over 50 | cost.rs 121-134, 187; cost_params.rs 107 | f9 A, f12 D | 09-cost-model |
| 13 | no adaptive or robust execution | join.rs 70, 1024; statement.rs 1040 | f32 A, f33 A, f34 A, f11 A | 10-no-adaptive |
| 14 | no upper bound on an estimate | join.rs 259-264; cost.rs 408-458 | f19 A, f40 C, f11 A | 04-statistics, 10-no-adaptive |
| 15 | estimation error measured per operator, but no q-error or P-error baseline yet | statement.rs 495; connection.rs 3969 | f36 B, f20 A, f37 A | the `-- counted` lines of 04, 05, 06, 08, 10, 11 |
| 16 | the greedy search, and the planner around it, hold unwrap, expect and panics | join.rs 1967, 2020, 2114; panics.out | f10 A | panics.out, 01-left-deep-greedy |

### Row 1. Left-deep trees only

- **Amphora.** Join orders are searched by a dynamic program over table subsets, size by size,
  each subset built from a subset one table smaller and one more table (join.rs line 1598: "for
  subset_size in 2..=num_tables {"; line 1617: "In this block, LHS is always (n-1) tables, and RHS
  is a single table."). Every plan is a loop nest, one table a level.
- **The leading engines.** PostgreSQL searches bushy trees without cross products exhaustively
  up to its collapse limits (f26, A); DuckDB enumerates bushy plans by DPhyp (f24, A); ClickHouse
  by DPsub and CockroachDB by DPSube (f25, A); Calcite by a Volcano dynamic program over
  rule-made alternatives (f29, A). On JOB with true cardinalities, left-deep trees cost up to 4.5
  times the best bushy plan (f27, A).
- **The gap.** No plan joins two composite inputs: `(a JOIN b) JOIN (c JOIN d)` runs as one
  pipeline, a table a step (`01-left-deep-greedy.out`, the last plan: "HASH JOIN b", "SEARCH c
  USING COVERING INDEX ephemeral_c_t3 (y=?)", "SEARCH d USING COVERING INDEX ephemeral_d_t4
  (z=?)", "SCAN a"). A hash join's build is one table (join.rs line 1333, the build's mask
  "[build_table_idx]"), never a join's result.
- **Benchmarks touched.** Every multi-join query: JOB's 113; TPC-H Q2, Q3, Q5, Q7 to Q10, Q18,
  Q21 (3 to 8 tables); TPC-DS Q64 (18 tables), Q72 (11), Q17, Q25, Q29, Q85 and Q88 (8) and the
  rest of its joins; SSB Q2.1 to Q4.3 (4 and 5 tables); CH-benCHmark Q2, Q5, Q7, Q8 and Q9 (6 to
  9). ClickBench reads one table and is not touched.

### Row 2. Above 12 tables, a greedy left-deep search called GOO

- **Amphora.** Above 12 tables (join.rs line 1854: "pub const GREEDY_JOIN_THRESHOLD: usize =
  12;"; line 1432: "if num_tables > GREEDY_JOIN_THRESHOLD {") the search is "Greedy Operator
  Ordering (GOO)" in name (line 1856), but it "Builds a left-deep join tree by:" (line 1858)
  adding, one at a time, the table of least cost at that step (line 1940: "Greedily add remaining
  tables, always picking lowest marginal cost."; line 1994: "if best.as_ref().is_none_or(|(_, b)|
  plan.cost < b.cost) {"). The cost compared is the plan's so far: a table's filter, which cuts
  the rows every later step reads, counts for nothing at the step it is added.
- **The leading engines.** GOO as published keeps a set of trees and merges the pair of least
  cost, so it builds bushy trees (f28, A). DuckDB runs DPhyp until 10,000 pairs, then GOO (f24,
  A). Neumann and Radke solve exactly within a budget of 10,000 connected subgraphs up to 100
  relations and linearize beyond (f23, A). On JOB, GOO's plans cost up to 21 times the optimum
  with true cardinalities (f27, A).
- **The gap.** A fact table and twelve dimensions, the last filtered (`01-left-deep-greedy.out`):
  with eleven (12 tables) the dynamic program reads the filtered dimension right after the fact
  table, total_cost 6,449,003.3; with twelve (13 tables) the greedy search reads it last, total_cost
  38,579,003.6, where the same 13 tables forced into the dynamic program's order cost 6,770,303.6:
  5.7 times more by Amphora's own model.
- **Benchmarks touched.** JOB 28a to 28c and 33a to 33c (14 tables) and 29a to 29c (17); TPC-DS
  Q64 (18). No query of TPC-H, SSB, CH-benCHmark or ClickBench joins more than 12 tables.

### Row 3. Subsets enumerated by size, not as connected pairs

- **Amphora.** The dynamic program walks every subset of each size (join.rs line 1599: "for mask
  in generate_join_bitmasks(num_tables, subset_size) {"; line 2523), and only then skips a
  join that isn't connected where a connected one exists (line 1634: "if
  has_connected_legal_candidate("). The subsets a chain of n tables can join without a cross
  product are n(n+1)/2 (78 for 12); the walk visits all 2^n - 1 (4,095).
- **The leading engines.** DPccp and DPhyp enumerate connected complement pairs only and meet the
  lower bound, where DPsize runs far above it (f22, A); DuckDB, ClickHouse and CockroachDB do
  (f24, f25, A). As published DPhyp is recursive; Amphora's rules require it as a loop over an
  explicit stack with a counted budget (f10, A).
- **The gap.** Planning time (`02-subsets-by-size.out`, the least of five prepares of EXPLAIN
  QUERY PLAN, on adacachy at a load average near 70 from other builds, so a diagnosis, not a
  benchmark record): a chain of 8 to 12 tables 0.097, 0.146, 0.247, 0.383 and 0.682 ms, 7.0 times
  from 8 to 12 where its connected subsets grow 2.2 times (36 to 78); a star of 8 to 12 tables
  0.802, 2.019, 5.875, 15.342 and 38.508 ms, 48 times. The search can't stay exact past 12 tables
  within a budget, which is why row 2's greedy search starts there.
- **Benchmarks touched.** Planning time of JOB's 28 queries of 10 to 12 tables and TPC-DS Q72
  (11); with a connected-pair search, the 9 JOB queries and TPC-DS Q64 above 12 could be planned
  exactly too.

### Row 4. No conflict detector

- **Amphora.** A LEFT or FULL JOIN's right table, and a semi- or anti-join's (EXISTS, NOT
  EXISTS, IN unnested), must follow every table written before it (join.rs lines 2029-2038: "LEFT/
  FULL OUTER, SEMI, and ANTI joins all require the RHS table"); a RIGHT or FULL JOIN also keeps
  every later table after its right table (line 2041: "A RIGHT or FULL JOIN is a reordering
  barrier in both directions.").
- **The leading engines.** ClickHouse and CockroachDB reorder outer, semi and anti joins exactly
  with Moerkotte, Fender and Eich's CD-C, "correct and complete" (f25, A); DPhyp handles non-inner
  joins in its hypergraph (f22, A).
- **The gap.** `(big RIGHT JOIN mid ON mid.k = big.k) JOIN tiny ON tiny.id = mid.id WHERE tiny.id
  = 5` equals `big RIGHT JOIN (mid JOIN tiny)`, since tiny's term reads only mid
  (`03-right-full-barrier.out`): with inner joins tiny's one row is read first, total_cost 23,007.0;
  with the RIGHT JOIN big and mid are read first, total_cost 328,300,039,000.3; the same query
  written as `mid JOIN tiny ... LEFT JOIN big` reads tiny first again, 23,007.0.
- **Benchmarks touched.** TPC-DS Q51 and Q97 (FULL JOINs of two inputs, where the barrier costs
  nothing) and the LEFT JOINs of TPC-H Q13, CH-benCHmark Q13 and TPC-DS Q5, Q40, Q49, Q72, Q75,
  Q77, Q78, Q80 and Q93; the semi- and anti-joins unnesting makes (row 10: TPC-H Q4 and Q22).
  JOB, SSB and ClickBench have none.

### Row 5. Statistics: sqlite_stat1 only, fallbacks without ANALYZE, independence

- **Amphora.** Statistics are `sqlite_stat1`'s (stats.rs line 10: "pub const STATS_TABLE: &str =
  "sqlite_stat1";"): a table's rows and, per index, the "Average number of rows per distinct key
  prefix, for each leftmost prefix" (line 18). No distinct count for a column no index leads, no
  histogram, no sample, no sketch. Without ANALYZE a table has "rows_per_table_fallback:
  1_000_000.0," (cost_params.rs line 106; cost.rs line 357), and filters fixed selectivities
  (cost_params.rs lines 110-119: an equality on a column no index leads 0.1, on an indexed one
  0.001, a range 0.4, a closed range 0.005, LIKE 0.2); after ANALYZE an equality on a column no
  index leads stays at 0.1 (constraints.rs lines 515-517: "params.sel_eq_unindexed"). A table's
  filters multiply (cost.rs line 429: "fraction *= constraint.selectivity;"), but for a closed
  range on one column (lines 448-449).
- **The leading engines.** Umbra and CedarDB keep a reservoir sample of every relation, updated
  online, and HyperLogLog sketches on every column (f14, f41, A); DuckDB HyperLogLog over a sample
  of each vector and join sizes from equivalence-class domains (f15, f39, A); ClickHouse t-digest,
  count-min and HyperLogLog per column, built by default (f42, A); PostgreSQL extended statistics
  by hand and SQL Server exponential backoff across predicates (f17, A). Samples catch
  correlations per-column statistics miss (f13, D, against Han et al.'s finding that sampling
  over joins did worse than PostgreSQL). On JOB every estimator erred by 1,000 times or more,
  multi-join results underestimated under independence (f11, A).
- **The gap.** On 10,000 rows (`04-statistics.out`): before ANALYZE, "output_rows": 1000000;
  after it, a = 1 AND b = 1 on two equal columns is estimated at 1 row and counted 100; flag = 1
  (two values, no index) estimated 1,000, counted 5,000; skew = 0 (9,900 rows) and skew = 9950
  (1 row) both estimated 100; u > 20000 estimated 4,000, counted 0.
- **Benchmarks touched.** Every query. Correlated predicates: JOB's join-crossing correlations
  (f13), TPC-H Q19's OR of conjunctions and Q6's three ranges, SSB Q1.1 to Q1.3 (ranges on
  lo_discount, lo_quantity and the date); ClickBench's filters on hits (its 43 queries read one
  table, most filter it, 29 group it); skew: ClickBench's columns and TPC-DS's (date_dim, item).

### Row 6. A join's estimate from the access path chosen (the open AG findings)

- **Amphora.** A join's rows are the input's times the access path's rows per outer row times its
  filters (join.rs lines 259-264: "input_cardinality * method.estimated_rows_per_outer_row *
  remaining_filter_selectivity * completed_selectivity"): an index seek's rows per outer row are
  its seek's (access_method.rs line 1116: "estimated_rows_per_outer_row: rows_per_seek,", from
  sqlite_stat1 by cost.rs line 457's estimate_rows_per_seek), a hash join's from its keys
  (access_method.rs line 1578), a scan's the table's rows (line 1917) with its terms then taken
  as filters at their fixed selectivities. The AF fix (f4) made a term's estimate independent of the
  order tables are read in; the estimate still depends on how a table is reached.
- **The leading engines.** PostgreSQL keeps one rowcount for every path of a join relation,
  CockroachDB gives a memo group's expressions one row count, DuckDB caches a cardinality per
  relation set (f16, f15, A). CERT checks a more restrictive query is never estimated at more
  rows (f21, A).
- **The gap.** r JOIN s ON s.k = r.k (2,000 rows each, 20,000 matches; `05-estimate-by-path.out`):
  through s's index on k, estimated 20,000 and counted 20,000; with s NOT INDEXED, 400,000; read s
  first by CROSS JOIN, 400,000. Campaign 2's cert-70d3e0baf8df5914 still fails: 10,000 rows, then
  320,000 with a conjunct added. Replaying every CERT finding the oracles recorded on f734df4
  (`cert-replay.out`, `cert_replay.py`, the oracle's own estimate rule): AF's 35 and P1 to P3's 4
  pass; of AG's 87, 82 fail, 4 have no estimate, 1 passes. Of the 82, 39 make an outer join inner
  (16 FULL, 13 RIGHT, 10 LEFT) and 43 add a conjunct (22 to inner joins alone, 17 beside a
  subquery, 4 to outer joins).
- **Benchmarks touched.** Every join: JOB's 113, TPC-H's 18 joining queries, TPC-DS's, SSB's
  13, CH-benCHmark's.

### Row 7. A FULL JOIN's estimate leaves out the right table's unmatched rows

- **Amphora.** An outer join's rows per input are the matches plus a Poisson chance of none
  (join.rs lines 286-288: "A Poisson model estimates the chance that an input row has no match.");
  that counts the left input's unmatched rows. Nothing adds the right table's rows that matched
  no left row, which a RIGHT or FULL JOIN emits in its pass after the loops. A hash FULL JOIN
  takes at least the probe's rows over the build's (access_method.rs lines 1582-1584).
- **The leading engines.** A FULL JOIN is never below either side's outer join (CERT's
  restriction monotonicity, f21, A); the set-based estimators give each join relation one row
  count (f16, A).
- **The gap.** l (100 rows) and r (1,000), no key matching (`06-full-join-estimate.out`): LEFT
  JOIN counted 100, RIGHT JOIN 1,000, FULL JOIN 1,100, each estimated 136.79; the inner join 100.
  The open AG findings that make a FULL JOIN inner (16 above) are this row's.
- **Benchmarks touched.** TPC-DS Q51 and Q97; no other benchmark has a RIGHT or FULL JOIN.

### Row 8. Join sizes: a fixed selectivity on columns no index leads

- **Amphora.** An equality on a column no index leads, a join term among them, is 0.1
  (constraints.rs lines 515-517; cost_params.rs line 110: "sel_eq_unindexed: 0.1,") with or
  without ANALYZE: a key column not declared one, and a join of two foreign keys, has no
  distinct count to divide by.
- **The leading engines.** DuckDB and Ebergen's thesis take every join as foreign key to primary
  key and divide by the equivalence class's domain, its largest distinct count (f15, f39, A); SQL
  Server assumes containment (f43, A).
- **The gap.** o (5,000 rows) JOIN c (500) ON c.ck = o.ck, ck unique in c but no index
  (`11-join-size.out`): estimated 250,000, counted 5,000.
- **Benchmarks touched.** Every join whose columns are loaded without an index: TPC-DS's as
  turso's schema declares it (no key and no index on any of its tables), SSB's star joins
  unless its keys are declared. The planner bench indexes every foreign key of TPC-H
  (`tpch/indexes.sql`) and JOB (`fkindexes.sql`), so there a join reaches row 6's seek
  estimates instead, and this row returns wherever the search picks a hash join or a scan.

### Row 9. A GROUP BY's groups are not estimated

- **Amphora.** A query's estimate is its join's last access (eqp.rs lines 950-960: "The rows the
  join is estimated to give"); grouping, DISTINCT and a derived table's groups are not estimated,
  so a grouped subquery joined to another table enters the join at its input's rows.
- **The leading engines.** CedarDB says its statistics "estimate relation size after group by
  operations" (f14, A).
- **The gap.** o's 5,000 rows grouped by ck (500 values): estimated 5,000 (`11-join-size.out`).
- **Benchmarks touched.** Where a group's result is joined or filtered: TPC-H Q13 (a derived
  table of counts), Q15 (a WITH of revenue), Q18 (IN over GROUP BY ... HAVING); TPC-DS's CTEs of
  aggregates joined to tables (Q1, Q30, Q81 among them, 82 of its queries group); ClickBench's 29
  GROUP BY queries and SSB's Q2 to Q4 for the sort after the groups.

### Row 10. Unnesting by fixed patterns

- **Amphora.** EXISTS and NOT EXISTS become semi- and anti-joins, a direct positive IN a
  semi-join, and a one-aggregate subquery is grouped by its correlation columns (unnest.rs lines
  1-7); "`NOT IN` stays as a subquery because NULL values can change its result." (line 106);
  nothing is unnested beside a RIGHT or FULL JOIN (lines 154-158); a semi-join's subquery must
  read one table (line 1267). The unnested form is kept only when estimated no costlier than the
  written one (mod.rs lines 1021-1035), each planned on its own ("Until then, both forms need
  their own search.", line 1001).
- **The leading engines.** Neumann and Kemper unnest every correlated subquery by dependent joins
  (f30, B); DuckDB uses it, CedarDB decorrelates fully (f31, A); Neumann's 2025 top-down variant,
  in Umbra, visits each operator once and turned PostgreSQL's 26 minutes on its Figure 1 query
  into 1.3 s (f38, C). As published both are recursive; Amphora would need them as a worklist
  loop (f10, A).
- **The gap.** Unnested (`13-tpch-subqueries.out`, TPC-H's own text and schema): Q4 (semi), Q22's
  NOT EXISTS (anti), Q2 and Q17 (grouped first), Q20's correlated sum (joined, then grouped).
  Kept: Q21's EXISTS and NOT EXISTS over its four outer tables, estimated cheaper as written (over
  two, `07-unnesting.out`, they are unnested). Uncorrelated, run once: Q11 and Q15's scalars, Q16,
  Q18 and Q20's IN lists. On TPC-DS (`tpcds-subqueries.out`): Q1, Q6, Q30, Q41 and Q81 keep a
  "CORRELATED SCALAR SUBQUERY", Q10, Q35 and Q69's EXISTS read two tables each and stay
  subqueries, Q16 and Q94's stay on cost; Q32 and Q92 are grouped first. Beside a RIGHT JOIN, Q4's
  EXISTS is a subquery (`07-unnesting.out`, no "join": "semi"); an OR of two EXISTS scans orders
  and lineitem for every customer; a correlated NOT IN is a "CORRELATED LIST SUBQUERY".
- **Benchmarks touched.** Kept: TPC-H Q21; TPC-DS Q1, Q6, Q10, Q16, Q30, Q35, Q41, Q69, Q81,
  Q94. Under the same rules, not run here: CH-benCHmark Q4, Q21 and Q22 (correlated EXISTS and
  NOT EXISTS, as TPC-H's) and Q16 (NOT IN). JOB, SSB and ClickBench have no subqueries.

### Row 11. Plans fixed at prepare

- **Amphora.** A statement is planned at prepare and again only when its run meets a schema
  change (statement.rs lines 709-735, `SchemaUpdated` to `reprepare`, line 1040). ANALYZE refreshes
  the connection's statistics (line 751) but re-plans no statement already prepared; a bound
  value is stored for the run (line 1453), never planned with.
- **The leading engines.** PostgreSQL and CockroachDB plan a prepared statement with its values
  for five runs, then generic if not significantly costlier; SQL Server keeps several plans by the
  parameters' cardinality; SQLite recompiles when a bound value could change the plan (f35, A).
- **The gap.** `08-fixed-at-prepare.out`: a join prepared before ANALYZE and run again after it
  keeps "output_rows": 100000000000 and its plan; the same text prepared after it, 5000. `WHERE k
  = ?` has one plan and one estimate, 1,000 rows, where k = 1 counts 1,000 and k = 9 none.
- **Benchmarks touched.** CH-benCHmark (its transactions prepared once while the analytical
  queries' tables grow under them), TPC-H and TPC-DS when their streams run prepared with
  substitution parameters; JOB, SSB and ClickBench run literal queries once.

### Row 12. A page-based cost model whose pages are rows over 50

- **Amphora.** Cost counts page reads, seeks, row, filter, hash and sort work in one unit (f9),
  table lookups by Mackert and Lohman (cost.rs line 187); a table's pages are its rows over a
  fixed 50 (cost.rs line 126: "let table_pages = (base_row_count /
  params.rows_per_table_page).max(1.0);"; cost_params.rs line 107: "rows_per_table_page: 50.0,"),
  not the pages it holds.
- **The leading engines.** On JOB, cost-model errors were "dwarfed by cardinality estimation
  errors"; DuckDB's join cost is each join's cardinality plus its inputs' (f12, D, graded so for
  Amphora's own counterclaim: its bench's plans did more work from cost gaps, fixed in planner.md
  decisions 2, 5 and 7). Calcite combines CPU, I/O and memory (f29, A).
- **The gap.** Two tables of 2,000 rows, one narrow, one of 2,000-byte rows
  (`09-cost-model.out`): the file grows from 8 pages to 1,011, both scans cost 46.
- **Benchmarks touched.** Wide rows: ClickBench's hits, TPC-DS's fact tables, JOB's text tables
  (movie_info, person_info), where a scan's cost is under-counted against a seek's.

### Row 13. No adaptive or robust execution

- **Amphora.** A plan runs as prepared (statement.rs line 1040 is the only path that plans
  again): no switch between nested loops and a hash join at run time, no re-optimization on rows
  counted, no Bloom filter passed between joins; a hash join is ruled out when its estimated
  build passes 200,000 rows (join.rs line 70: "const MAX_MATERIALIZED_BUILD_ROWS: f64 =
  200_000.0;", line 1024).
- **The leading engines.** SQL Server's adaptive join picks hash or nested loops after its first
  input against a threshold; on JOB, removing nested loops without an index ended the timeouts
  (f32, A). LIP, predicate transfer, POLAR and diamond-hardened joins make plans robust to join
  order (f33, A); mid-query re-optimization cut JOB's 20 slowest queries by 27% (f34, A).
- **The gap.** p and q analyzed at 10 rows, then grown to 3,000 (`10-no-adaptive.out`): estimated
  10 and 10, counted 3,000 and 3,000, the plan "SCAN p" with "SCAN q" inside it, a full scan of q
  for every row of p.
- **Benchmarks touched.** JOB (estimates off by 1,000 times, f11); star joins where Bloom filters
  pay: SSB Q2.1 to Q4.3, TPC-H Q5, Q8 and Q9, TPC-DS's store_sales and catalog_sales stars;
  CH-benCHmark, whose data grows past its statistics while it runs.

### Row 14. No upper bound on an estimate

- **Amphora.** Selectivities multiply without a bound (join.rs lines 259-264; cost.rs lines
  408-458): no estimate is capped by what statistics guarantee.
- **The leading engines.** LpBound and SafeBound bound join outputs from degree sequences and
  never underestimate; with several predicates on a table SafeBound takes the most selective
  predicate's cardinality (f19, A; f40, C).
- **The gap.** a = 1 AND b = 1 (`04-statistics.out`): each index guarantees 100 rows a value, the
  bound SafeBound would give, and the estimate is 1 against 100 counted; row 13's join is
  estimated 10 against 3,000.
- **Benchmarks touched.** JOB (multi-join underestimates, f11); TPC-H, TPC-DS and SSB's
  multi-predicate dimension filters.

### Row 15. Estimation error: counted per operator, no baseline yet

- **Amphora.** Since 016d245 a statement counts the rows at each table access and join when asked
  (statement.rs line 495: "pub fn row_counts(&self)"; connection.rs line 3969: "pub fn
  set_row_counters"), closing what f36 found missing; q-error and P-error over JOB and TPC-H are
  step 3's baseline, not yet recorded.
- **The leading engines and sources.** q-error bounds a plan's cost; P-error tracks end-to-end
  time better (f20, A); the workloads are at hand (f37, A).
- **The gap.** Every `-- counted` line of the demonstrations is an estimate beside its count
  (04, 05, 06, 08, 10, 11); the benchmark-wide distribution waits on step 3.
- **Benchmarks touched.** JOB's 113 and TPC-H's 22 (step 3); the others once they are loaded.

### Row 16. Panicking calls in the greedy search and the planner

- **Amphora.** CLAUDE.md section 1 forbids unwrap, expect, panic and unreachable in production
  code (f10, A). The greedy search holds `join_order.last_mut().unwrap()` (join.rs line 1967),
  `current_plan.expect("loop invariant: current_plan always Some")` (line 2020) and, in its
  starting table, `best.expect("no valid starting table")` (line 2114); the join search beside it
  two unwraps (lines 612, 695), and its hash join an expect (1333) and an unreachable (1003). The planner's files
  (`panics.out`, from scripts/check-production.sh's count pass on f734df4) hold 26 unwrap, 54
  expect, 8 panic, 10 unreachable, 253 slice indexings and 67 unchecked arithmetic: join.rs 3, 3,
  0, 1, 47, 14; mod.rs 7, 11, 5, 3, 89, 13; constraints.rs 9, 7, 3, 1, 39, 3; unnest.rs 0, 6, 0,
  3, 15, 6; statement.rs 2, 14, 0, 0, 3, 2; stats.rs 1, 0, 0, 0, 4, 6.
- **The leading engines.** Not in the research; the bar is Amphora's own.
- **The gap.** Any change to the search, the estimator or the unnesting replaces the calls it
  touches with typed errors, and lowers scripts/engine-ratchet.toml with them.
- **Benchmarks touched.** Every query is planned through mod.rs and join.rs; the greedy search's
  three calls run for JOB 28a to 29c and 33a to 33c and TPC-DS Q64.

### The research with no row

- **Learned estimators** (f18, D): more accurate on static data, costly to train and to keep with
  updates, and Han et al. find them improving PostgreSQL where traditional methods hardly do; no
  gap is claimed until a measurement on Amphora's workloads says which.
- **Calcite's rule engines** (f29, A): a framework's design, not a technique Amphora lacks for
  these benchmarks.
- **The rules on how a change is built** (f10, A): no recursion, counted budgets; they bind rows
  1 to 3 and 10 (DPhyp and both unnesting papers are stated recursively).

## What would falsify these notes

- A finding's quote missing from its source at the stated location. Kiln re-checks each quote. The
  revisions above fix where the code quotes are.
- For the engines' code: a later revision changing the cited lines (DuckDB's 10,000-pair cutoff or
  12-relation threshold, ClickHouse's `dpsub,greedy` default, its 0.33 and 0.01 factors, or
  CockroachDB's 8-join limit). The table states what these revisions do, not what later ones do.
- For the papers: a replication on JOB or STATS-CEB contradicting a headline result (estimates over
  costs, f12, already has one from Amphora's own workload; learned estimators, f18, have Han et
  al.'s).
- For the audit (section 7): an output of `bench/planner/audit/run.sh --check` differing from the
  one kept, on f734df4; a file and line it cites at f734df4 not showing what its row says; or a
  benchmark query its rows name whose text lacks the shape named (`benchmarks.tsv` is a scan of
  the text, checked by hand where cited).
