-- TPC-H's own decimals: prices and discounts.
SELECT sum(l_discount), avg(l_discount), total(l_discount), sum(l_extendedprice * (1 - l_discount)), avg(l_extendedprice) FROM lineitem;
