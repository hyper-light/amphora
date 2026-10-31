CREATE INDEX nation_regionkey ON nation (n_regionkey);
CREATE INDEX supplier_nationkey ON supplier (s_nationkey);
CREATE INDEX partsupp_suppkey ON partsupp (ps_suppkey);
CREATE INDEX customer_nationkey ON customer (c_nationkey);
CREATE INDEX orders_custkey ON orders (o_custkey);
CREATE INDEX lineitem_partkey_suppkey ON lineitem (l_partkey, l_suppkey);
CREATE INDEX lineitem_suppkey ON lineitem (l_suppkey);
