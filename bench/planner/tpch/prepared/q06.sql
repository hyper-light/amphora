select sum(l_extendedprice * l_discount) as revenue
from lineitem
where l_shipdate >= ?1 and l_shipdate < ?2
  and l_discount between ?3 and ?4 and l_quantity < ?5
