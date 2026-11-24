-- Past the double's range: every input 1e308; and +1e308 +1e308 -1e308 -1e308 within each order.
SELECT sum(1e308), avg(1e308), total(1e308), sum(CASE l_linenumber WHEN 1 THEN 1e308 WHEN 2 THEN 1e308 WHEN 3 THEN -1e308 WHEN 4 THEN -1e308 ELSE 0.0 END) FROM lineitem;
