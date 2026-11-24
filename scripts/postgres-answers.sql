-- PostgreSQL 19's answers for crates/pg-analysis/tests (docs/design/postgres.md section 11's unit 8),
-- run by scripts/postgres-answers.sh on a 19 server. Writes /tmp/operators.txt, /tmp/casts.txt,
-- /tmp/functions.txt and /tmp/statements.out there, the last from /tmp/statements.in.
\set ON_ERROR_STOP 1
set client_min_messages = error;
drop schema if exists sw cascade;
create schema sw;
drop table if exists pgbench_accounts, pgbench_history, t;
create table pgbench_accounts (aid int not null, bid int, abalance int, filler char(84));
create table pgbench_history (tid int, bid int, aid int, delta int, mtime timestamp, filler char(22));
create table t (i4 int4, i8 int8, tx text, bp char(5), ch "char", ts timestamp, tz timestamptz, rc regclass);

-- Each operator name between each pair of the unit's types: the operator's OID, or the SQLSTATE.
create table sw.operators (o text, l text, r text, a text);
do $$
declare
  o record; l text; r text; a text; lt text; rt text;
  ts text[] := array['int4', 'int8', 'text', 'bpchar', '"char"', 'timestamp', 'timestamptz', 'regclass', 'unknown'];
begin
  for o in select distinct oprname, oprkind from pg_operator order by 1, 2 loop
    foreach r in array ts loop
      rt := case when r = 'unknown' then 'NULL' else 'NULL::' || r end;
      foreach l in array (case when o.oprkind = 'b' then ts else array['-'] end) loop
        lt := case when l = '-' then '' when l = 'unknown' then 'NULL' else 'NULL::' || l end;
        begin
          execute 'create temp view v as select (' || lt || ' operator(pg_catalog.' || o.oprname || ') ' || rt || ') as x';
          select (regexp_match(ev_action, ':opno (\d+)'))[1] into a from pg_rewrite where ev_class = 'v'::regclass;
          drop view v;
        exception when others then
          get stacked diagnostics a = returned_sqlstate;
        end;
        insert into sw.operators values (o.oprname, l, r, a);
      end loop;
    end loop;
  end loop;
end $$;
\copy (select o || ' ' || l || ' ' || r || ' ' || a from sw.operators where a <> '42883' order by o collate "C", l collate "C", r collate "C") to '/tmp/operators.txt'

-- Each cast: the explicit one's node, whether it is allowed as an assignment, and implicitly.
create table sw.casts (s text, t text, e text, a text, i text);
do $$
declare
  s text; t text; e text; a text; i text; n int := 0; st text; node text;
  source text[] := array['int4', 'int8', 'text', 'bpchar', '"char"', 'timestamp', 'timestamptz', 'regclass', 'unknown'];
  target text[] := array['int4', 'int8', 'text', 'bpchar', '"char"', 'timestamp', 'timestamptz', 'regclass', 'bool', 'oid',
    'int2', 'numeric', 'float8', 'varchar', 'name', 'date', 'time', 'interval', 'bytea', '_int4', '_text', 'oidvector', 'uuid', 'json'];
begin
  foreach t in array target loop
    n := n + 1;
    execute 'create table sw.t' || n || ' (c ' || t || ')';
    execute 'create function sw.f' || n || '(' || t || ') returns int language sql return 1';
    foreach s in array source loop
      st := case when s = 'unknown' then 'NULL' else 'NULL::' || s end;
      begin
        execute 'create temp view v as select (' || st || '::' || t || ') as x';
        select substring(ev_action from ':targetList \(\{TARGETENTRY :expr (\{[A-Z]+ [^{]*)') into node
          from pg_rewrite where ev_class = 'v'::regclass;
        drop view v;
        e := case when s = t then 'C' when node like '{RELABELTYPE%' then 'R' when node like '{COERCEVIAIO%' then 'I'
          when node like '{ARRAYCOERCE%' then 'A' when node like '{CONST%' then 'C'
          when node like '{FUNCEXPR%' then 'F' || (regexp_match(node, ':funcid (\d+)'))[1] end;
      exception when others then
        get stacked diagnostics e = returned_sqlstate;
      end;
      begin
        execute 'prepare p as insert into sw.t' || n || ' values (' || st || ')';
        execute 'deallocate p';
        a := 'ok';
      exception when others then
        get stacked diagnostics a = returned_sqlstate;
      end;
      begin
        execute 'prepare p as select sw.f' || n || '(' || st || ')';
        execute 'deallocate p';
        i := 'ok';
      exception when others then
        get stacked diagnostics i = returned_sqlstate;
      end;
      insert into sw.casts values (s, t, e, a, i);
    end loop;
  end loop;
end $$;
\copy (select s || ' ' || t || ' ' || e || ' ' || a || ' ' || i from sw.casts order by s collate "C", t collate "C") to '/tmp/casts.txt'

-- Each function of one argument of the unit's types, by name, and of two where several of one name take
-- two: the function's OID (`coercion` for a cast written as a call), or the SQLSTATE where none is chosen.
-- The functions are pg_proc.dat's: one from FirstUnpinnedObjectId (12000, src/include/access/transam.h) on
-- is an extension's that initdb installs (plpgsql) or one of its scripts' (snowball's).
create table sw.functions (f text, l text, r text, a text);
do $$
declare
  p record; l text; r text; a text; lt text; rt text; node text;
  ts text[] := array['int4', 'int8', 'text', 'bpchar', '"char"', 'timestamp', 'timestamptz', 'regclass', 'unknown'];
begin
  for p in select proname, bool_or(n > 1 and pronargs = 2) as two, bool_or(pronargs - pronargdefaults <= 1 and pronargs >= 1) as one
      from (select *, count(*) over (partition by proname, pronargs) as n from pg_proc where pronamespace = 11 and oid < 12000 and proname !~ '^RI_') x group by 1 order by 1 loop
    foreach l in array (case when p.one or p.two then ts else '{}' end) loop
      lt := case when l = 'unknown' then 'NULL' else 'NULL::' || l end;
      foreach r in array (case when p.one and p.two then array['-'] || ts when p.two then ts else array['-'] end) loop
        rt := case when r = '-' then '' when r = 'unknown' then ', NULL' else ', NULL::' || r end;
        begin
          execute format('create temp view v as select pg_catalog.%I(%s%s) as x', p.proname, lt, rt);
          select substring(ev_action from ':targetList \(\{TARGETENTRY :expr (\{[A-Z]+ [^{]*)') into node
            from pg_rewrite where ev_class = 'v'::regclass;
          drop view v;
          a := coalesce((regexp_match(node, '^\{(?:FUNCEXPR :funcid|AGGREF :aggfnoid|WINDOWFUNC :winfnoid) (\d+)'))[1], 'coercion');
        exception when others then
          get stacked diagnostics a = returned_sqlstate;
        end;
        insert into sw.functions values (p.proname, l, r, a);
      end loop;
    end loop;
  end loop;
end $$;
\copy (select f || ' ' || l || ' ' || r || ' ' || a from sw.functions where a <> '42883' order by f collate "C", l collate "C", r collate "C") to '/tmp/functions.txt'
select 'functions asked', count(*), count(*) filter (where a <> '42883') from sw.functions;

-- Each statement under its TimeZone: PREPARE's SQLSTATE, or its parameters' and columns' types (with
-- their type modifiers where it has no parameters), then its value: what it selects or stores.
create function sw.names(types regtype[]) returns text language sql as $$
  select coalesce(string_agg(t.typname, ',' order by n), '') from unnest(types) with ordinality u (o, n) join pg_type t on t.oid = u.o
$$;
create function sw.answer(zone text, q text, args text) returns text language plpgsql as $$
declare
  types text; results text; v text; st text; col text;
begin
  execute format('set local timezone = %L', zone);
  execute 'prepare p as ' || q;
  select sw.names(parameter_types), sw.names(result_types) into types, results from pg_prepared_statements where name = 'p';
  begin
    if types <> '' then
      if args <> '' then
        execute format('execute p(%s)', (select string_agg(quote_literal(x), ',') from unnest(string_to_array(args, ';')) x)) into v;
        v := coalesce(v, 'NULL');
      end if;
    elsif q ~* '^select' then
      execute 'create temp view v as ' || q;
      select string_agg(t.typname || case when atttypmod < 0 then '' else ':' || atttypmod end, ',' order by attnum) into results
        from pg_attribute a join pg_type t on t.oid = a.atttypid where attrelid = 'v'::regclass and attnum > 0;
      drop view v;
      if q !~* ' from ' and results !~ ',' and q !~* 'current_timestamp' then
        execute 'select coalesce(format(''%s'', x.c), ''NULL'') from (' || q || ') x(c)' into v;
      end if;
    elsif q ~* '^insert into t\(\w+\) values' then
      execute q;
      execute 'select coalesce(format(''%s'', ' || substring(q from '\((\w+)\)') || '), ''NULL'') from t' into v;
      delete from t;
    elsif q ~ '^UPDATE t SET .* WHERE true$' then
      insert into t default values;
      execute q;
      execute 'select coalesce(format(''%s'', ' || substring(q from 'SET (\w+)') || '), ''NULL'') from t' into v;
      delete from t;
    end if;
  exception when others then
    get stacked diagnostics v = returned_sqlstate;
  end;
  execute 'deallocate p';
  return types || ' -> ' || results || coalesce(' = ' || v, '');
exception when others then
  get stacked diagnostics st = returned_sqlstate;
  return st;
end $$;
create table sw.statements (n int generated always as identity, zone text, q text, args text);
\copy sw.statements (zone, q, args) from '/tmp/statements.in' (format csv, delimiter '|', quote E'\x01', null '')
\copy (select zone || '|' || q || '|' || coalesce(args, '') || '|' || sw.answer(zone, q, coalesce(args, '')) from sw.statements order by n) to '/tmp/statements.out' (format csv, delimiter E'\x02', quote E'\x01')
