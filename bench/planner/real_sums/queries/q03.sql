-- Tenths with no exact double: 0.1 + 0.2 - 0.3 (sqlite.org/floatingpoint.html).
SELECT sum(CASE l_linenumber WHEN 1 THEN 0.1 WHEN 2 THEN 0.2 WHEN 3 THEN -0.3 ELSE 0.0 END), avg(CASE l_linenumber WHEN 1 THEN 0.1 WHEN 2 THEN 0.2 WHEN 3 THEN -0.3 ELSE 0.0 END), total(CASE l_linenumber WHEN 1 THEN 0.1 WHEN 2 THEN 0.2 WHEN 3 THEN -0.3 ELSE 0.0 END) FROM lineitem;
