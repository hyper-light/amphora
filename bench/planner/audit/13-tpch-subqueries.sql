-- Audit row 10 on TPC-H: EXPLAIN QUERY PLAN FORMAT=JSON of every TPC-H query holding a subquery
-- (Q2, Q4, Q11, Q15, Q16, Q17, Q18, Q20, Q21, Q22), on the planner bench's schema and indexes as
-- Amphora gets them (tpch/schema.sql, {DEC} REAL and {DATE} TEXT; tpch/indexes.sql; tpch/queries,
-- {FRACTION} SF 1's 0.0001), no rows, no ANALYZE. A semi- or anti-join is an access with
-- "join": "semi" or "anti", a grouped-first subquery a scalar_subquery table, a kept one a SUBQUERY node.
CREATE TABLE region (r_regionkey INTEGER NOT NULL PRIMARY KEY, r_name CHAR(25) NOT NULL, r_comment VARCHAR(152));
CREATE TABLE nation (n_nationkey INTEGER NOT NULL PRIMARY KEY, n_name CHAR(25) NOT NULL, n_regionkey INTEGER NOT NULL, n_comment VARCHAR(152));
CREATE TABLE part (p_partkey INTEGER NOT NULL PRIMARY KEY, p_name VARCHAR(55) NOT NULL, p_mfgr CHAR(25) NOT NULL, p_brand CHAR(10) NOT NULL, p_type VARCHAR(25) NOT NULL, p_size INTEGER NOT NULL, p_container CHAR(10) NOT NULL, p_retailprice REAL NOT NULL, p_comment VARCHAR(23) NOT NULL);
CREATE TABLE supplier (s_suppkey INTEGER NOT NULL PRIMARY KEY, s_name CHAR(25) NOT NULL, s_address VARCHAR(40) NOT NULL, s_nationkey INTEGER NOT NULL, s_phone CHAR(15) NOT NULL, s_acctbal REAL NOT NULL, s_comment VARCHAR(101) NOT NULL);
CREATE TABLE partsupp (ps_partkey INTEGER NOT NULL, ps_suppkey INTEGER NOT NULL, ps_availqty INTEGER NOT NULL, ps_supplycost REAL NOT NULL, ps_comment VARCHAR(199) NOT NULL, PRIMARY KEY (ps_partkey, ps_suppkey));
CREATE TABLE customer (c_custkey INTEGER NOT NULL PRIMARY KEY, c_name VARCHAR(25) NOT NULL, c_address VARCHAR(40) NOT NULL, c_nationkey INTEGER NOT NULL, c_phone CHAR(15) NOT NULL, c_acctbal REAL NOT NULL, c_mktsegment CHAR(10) NOT NULL, c_comment VARCHAR(117) NOT NULL);
CREATE TABLE orders (o_orderkey INTEGER NOT NULL PRIMARY KEY, o_custkey INTEGER NOT NULL, o_orderstatus CHAR(1) NOT NULL, o_totalprice REAL NOT NULL, o_orderdate TEXT NOT NULL, o_orderpriority CHAR(15) NOT NULL, o_clerk CHAR(15) NOT NULL, o_shippriority INTEGER NOT NULL, o_comment VARCHAR(79) NOT NULL);
CREATE TABLE lineitem (l_orderkey INTEGER NOT NULL, l_partkey INTEGER NOT NULL, l_suppkey INTEGER NOT NULL, l_linenumber INTEGER NOT NULL, l_quantity REAL NOT NULL, l_extendedprice REAL NOT NULL, l_discount REAL NOT NULL, l_tax REAL NOT NULL, l_returnflag CHAR(1) NOT NULL, l_linestatus CHAR(1) NOT NULL, l_shipdate TEXT NOT NULL, l_commitdate TEXT NOT NULL, l_receiptdate TEXT NOT NULL, l_shipinstruct CHAR(25) NOT NULL, l_shipmode CHAR(10) NOT NULL, l_comment VARCHAR(44) NOT NULL, PRIMARY KEY (l_orderkey, l_linenumber));
CREATE INDEX nation_regionkey ON nation (n_regionkey);
CREATE INDEX supplier_nationkey ON supplier (s_nationkey);
CREATE INDEX partsupp_suppkey ON partsupp (ps_suppkey);
CREATE INDEX customer_nationkey ON customer (c_nationkey);
CREATE INDEX orders_custkey ON orders (o_custkey);
CREATE INDEX lineitem_partkey_suppkey ON lineitem (l_partkey, l_suppkey);
CREATE INDEX lineitem_suppkey ON lineitem (l_suppkey);
-- TPC-H Q02
EXPLAIN QUERY PLAN FORMAT=JSON select s_acctbal, s_name, n_name, p_partkey, p_mfgr, s_address, s_phone, s_comment from part, supplier, partsupp, nation, region where p_partkey = ps_partkey and s_suppkey = ps_suppkey and p_size = 15 and p_type like '%BRASS'   and s_nationkey = n_nationkey and n_regionkey = r_regionkey and r_name = 'EUROPE'   and ps_supplycost = (     select min(ps_supplycost) from partsupp, supplier, nation, region     where p_partkey = ps_partkey and s_suppkey = ps_suppkey and s_nationkey = n_nationkey       and n_regionkey = r_regionkey and r_name = 'EUROPE') order by s_acctbal desc, n_name, s_name, p_partkey limit 100;
-- TPC-H Q04
EXPLAIN QUERY PLAN FORMAT=JSON select o_orderpriority, count(*) as order_count from orders where o_orderdate >= '1993-07-01' and o_orderdate < '1993-10-01'   and exists (select * from lineitem where l_orderkey = o_orderkey and l_commitdate < l_receiptdate) group by o_orderpriority order by o_orderpriority;
-- TPC-H Q11
EXPLAIN QUERY PLAN FORMAT=JSON select ps_partkey, sum(ps_supplycost * ps_availqty) as value from partsupp, supplier, nation where ps_suppkey = s_suppkey and s_nationkey = n_nationkey and n_name = 'GERMANY' group by ps_partkey having sum(ps_supplycost * ps_availqty) > (   select sum(ps_supplycost * ps_availqty) * 0.0001   from partsupp, supplier, nation   where ps_suppkey = s_suppkey and s_nationkey = n_nationkey and n_name = 'GERMANY') order by value desc;
-- TPC-H Q15
EXPLAIN QUERY PLAN FORMAT=JSON with revenue0 as (   select l_suppkey as supplier_no, sum(l_extendedprice * (1 - l_discount)) as total_revenue   from lineitem   where l_shipdate >= '1996-01-01' and l_shipdate < '1996-04-01'   group by l_suppkey) select s_suppkey, s_name, s_address, s_phone, total_revenue from supplier, revenue0 where s_suppkey = supplier_no and total_revenue = (select max(total_revenue) from revenue0) order by s_suppkey;
-- TPC-H Q16
EXPLAIN QUERY PLAN FORMAT=JSON select p_brand, p_type, p_size, count(distinct ps_suppkey) as supplier_cnt from partsupp, part where p_partkey = ps_partkey and p_brand <> 'Brand#45' and p_type not like 'MEDIUM POLISHED%'   and p_size in (49, 14, 23, 45, 19, 3, 36, 9)   and ps_suppkey not in (select s_suppkey from supplier where s_comment like '%Customer%Complaints%') group by p_brand, p_type, p_size order by supplier_cnt desc, p_brand, p_type, p_size;
-- TPC-H Q17
EXPLAIN QUERY PLAN FORMAT=JSON select sum(l_extendedprice) / 7.0 as avg_yearly from lineitem, part where p_partkey = l_partkey and p_brand = 'Brand#23' and p_container = 'MED BOX'   and l_quantity < (select 0.2 * avg(l_quantity) from lineitem where l_partkey = p_partkey);
-- TPC-H Q18
EXPLAIN QUERY PLAN FORMAT=JSON select c_name, c_custkey, o_orderkey, o_orderdate, o_totalprice, sum(l_quantity) from customer, orders, lineitem where o_orderkey in (select l_orderkey from lineitem group by l_orderkey having sum(l_quantity) > 300)   and c_custkey = o_custkey and o_orderkey = l_orderkey group by c_name, c_custkey, o_orderkey, o_orderdate, o_totalprice order by o_totalprice desc, o_orderdate limit 100;
-- TPC-H Q20
EXPLAIN QUERY PLAN FORMAT=JSON select s_name, s_address from supplier, nation where s_suppkey in (     select ps_suppkey from partsupp     where ps_partkey in (select p_partkey from part where p_name like 'forest%')       and ps_availqty > (         select 0.5 * sum(l_quantity) from lineitem         where l_partkey = ps_partkey and l_suppkey = ps_suppkey           and l_shipdate >= '1994-01-01' and l_shipdate < '1995-01-01'))   and s_nationkey = n_nationkey and n_name = 'CANADA' order by s_name;
-- TPC-H Q21
EXPLAIN QUERY PLAN FORMAT=JSON select s_name, count(*) as numwait from supplier, lineitem l1, orders, nation where s_suppkey = l1.l_suppkey and o_orderkey = l1.l_orderkey and o_orderstatus = 'F'   and l1.l_receiptdate > l1.l_commitdate   and exists (select * from lineitem l2 where l2.l_orderkey = l1.l_orderkey and l2.l_suppkey <> l1.l_suppkey)   and not exists (select * from lineitem l3 where l3.l_orderkey = l1.l_orderkey     and l3.l_suppkey <> l1.l_suppkey and l3.l_receiptdate > l3.l_commitdate)   and s_nationkey = n_nationkey and n_name = 'SAUDI ARABIA' group by s_name order by numwait desc, s_name limit 100;
-- TPC-H Q22
EXPLAIN QUERY PLAN FORMAT=JSON select cntrycode, count(*) as numcust, sum(c_acctbal) as totacctbal from (   select substr(c_phone, 1, 2) as cntrycode, c_acctbal   from customer   where substr(c_phone, 1, 2) in ('13', '31', '23', '29', '30', '18', '17')     and c_acctbal > (select avg(c_acctbal) from customer       where c_acctbal > 0.00 and substr(c_phone, 1, 2) in ('13', '31', '23', '29', '30', '18', '17'))     and not exists (select * from orders where o_custkey = c_custkey) ) as custsale group by cntrycode order by cntrycode;
