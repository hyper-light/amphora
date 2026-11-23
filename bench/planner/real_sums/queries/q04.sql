-- Mixed magnitudes: huge, tiny, the huge taken back, the smallest subnormal.
SELECT sum(CASE l_linenumber WHEN 1 THEN 1e300 WHEN 2 THEN 1e-300 WHEN 3 THEN -1e300 ELSE 5e-324 END), avg(CASE l_linenumber WHEN 1 THEN 1e300 WHEN 2 THEN 1e-300 WHEN 3 THEN -1e300 ELSE 5e-324 END), total(CASE l_linenumber WHEN 1 THEN 1e300 WHEN 2 THEN 1e-300 WHEN 3 THEN -1e300 ELSE 5e-324 END) FROM lineitem;
