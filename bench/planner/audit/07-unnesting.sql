-- Audit row 7: unnesting by fixed patterns (unnest.rs): EXISTS, NOT EXISTS and a direct positive IN
-- become semi- and anti-joins, a one-aggregate subquery is grouped by its correlation columns; the
-- rest stays a subquery run for each outer row. TPC-H's tables, no rows, no ANALYZE.
CREATE TABLE part(p_partkey INTEGER PRIMARY KEY, p_brand TEXT, p_type TEXT, p_size INT, p_name TEXT);
CREATE TABLE supplier(s_suppkey INTEGER PRIMARY KEY, s_name TEXT, s_comment TEXT, s_nationkey INT);
CREATE TABLE partsupp(ps_partkey INT, ps_suppkey INT, ps_availqty INT, ps_supplycost REAL, PRIMARY KEY (ps_partkey, ps_suppkey));
CREATE TABLE orders(o_orderkey INTEGER PRIMARY KEY, o_custkey INT, o_orderdate TEXT);
CREATE TABLE lineitem(l_orderkey INT, l_linenumber INT, l_partkey INT, l_suppkey INT, l_quantity REAL, l_commitdate TEXT, l_receiptdate TEXT, PRIMARY KEY (l_orderkey, l_linenumber));
CREATE TABLE customer(c_custkey INTEGER PRIMARY KEY, c_phone TEXT, c_acctbal REAL);
CREATE TABLE nation(n_nationkey INTEGER PRIMARY KEY, n_name TEXT, n_regionkey INT);
CREATE TABLE region(r_regionkey INTEGER PRIMARY KEY, r_name TEXT);
-- Unnested: Q4's EXISTS, a semi-join (lineitem's access has "join": "semi").
EXPLAIN QUERY PLAN FORMAT=JSON SELECT count(*) FROM orders WHERE EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey AND l_commitdate < l_receiptdate);
-- Kept: the same EXISTS beside a RIGHT JOIN (unnest.rs rewrite_correlated_subqueries returns at
-- has_right_or_full_join): lineitem is read by the subquery, no semi-join.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT count(*) FROM orders RIGHT JOIN customer ON c_custkey = o_custkey WHERE EXISTS (SELECT * FROM lineitem WHERE l_orderkey = o_orderkey AND l_commitdate < l_receiptdate);
-- Kept: an OR of correlated EXISTS (TPC-DS Q10 and Q35): each scans its tables for every customer.
EXPLAIN QUERY PLAN SELECT count(*) FROM customer WHERE EXISTS (SELECT * FROM orders WHERE o_custkey = c_custkey AND o_orderdate > '1995') OR EXISTS (SELECT * FROM lineitem JOIN orders ON o_orderkey = l_orderkey WHERE o_custkey = c_custkey);
-- Kept: Q16's NOT IN, a list built once; correlated, a list built for every outer row.
EXPLAIN QUERY PLAN SELECT p_brand, count(DISTINCT ps_suppkey) FROM partsupp JOIN part ON p_partkey = ps_partkey WHERE p_size IN (49, 14) AND ps_suppkey NOT IN (SELECT s_suppkey FROM supplier WHERE s_comment LIKE '%Customer%Complaints%') GROUP BY p_brand;
EXPLAIN QUERY PLAN SELECT count(*) FROM customer WHERE c_custkey NOT IN (SELECT o_custkey FROM orders WHERE o_orderdate > c_phone);
-- Unnested, for contrast: Q21's EXISTS and NOT EXISTS (correlated by = and <>) over two of its
-- tables (over all four, 13-tpch-subqueries.out keeps both, the written form estimated cheaper),
-- Q17's correlated avg and Q2's correlated min over four tables (grouped first), and a count in
-- the SELECT list.
EXPLAIN QUERY PLAN SELECT count(*) FROM lineitem l1 JOIN orders ON o_orderkey = l1.l_orderkey WHERE EXISTS (SELECT * FROM lineitem l2 WHERE l2.l_orderkey = l1.l_orderkey AND l2.l_suppkey <> l1.l_suppkey) AND NOT EXISTS (SELECT * FROM lineitem l3 WHERE l3.l_orderkey = l1.l_orderkey AND l3.l_suppkey <> l1.l_suppkey AND l3.l_receiptdate > l3.l_commitdate);
EXPLAIN QUERY PLAN SELECT sum(l_quantity) FROM lineitem JOIN part ON p_partkey = l_partkey WHERE p_brand = 'Brand#23' AND l_quantity < (SELECT 0.2 * avg(l_quantity) FROM lineitem WHERE l_partkey = p_partkey);
EXPLAIN QUERY PLAN SELECT s_name FROM part, supplier, partsupp, nation, region WHERE p_partkey = ps_partkey AND s_suppkey = ps_suppkey AND p_size = 15 AND s_nationkey = n_nationkey AND n_regionkey = r_regionkey AND r_name = 'EUROPE' AND ps_supplycost = (SELECT min(ps_supplycost) FROM partsupp, supplier, nation, region WHERE p_partkey = ps_partkey AND s_suppkey = ps_suppkey AND s_nationkey = n_nationkey AND n_regionkey = r_regionkey AND r_name = 'EUROPE');
EXPLAIN QUERY PLAN SELECT c_custkey, (SELECT count(*) FROM orders WHERE o_custkey = c_custkey) FROM customer;
