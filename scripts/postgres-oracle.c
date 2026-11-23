/*
 * The parser oracle of docs/design/postgres.md section 3 (unit 3 of section 11): PostgreSQL 19's own
 * raw_parser, unpatched, linked from scripts/postgres-oracle-extract.sh's extraction of REL_19_0.
 * Test-only, run as a subprocess by crates/pg-oracle so the Rust side has no `unsafe`.
 *
 * stdin: statements, each ended by a NUL byte. stdout, one frame a statement:
 *   T <bytes>\n<nodeToStringWithLocations of raw_parser's List of RawStmt>\n
 *   E <SQLSTATE> <cursor> <bytes>\n<message>\n
 * The cursor is the error's character position, 1-based, 0 for none (ErrorResponse's P field).
 */
#include "postgres.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "mb/pg_wchar.h"
#include "miscadmin.h"
#include "nodes/nodes.h"
#include "parser/parser.h"
#include "utils/guc_hooks.h"
#include "utils/memutils.h"

static void
frame(char kind, const char *head, const char *body)
{
	size_t		len = strlen(body);

	printf("%c %s%s%zu\n", kind, head, head[0] ? " " : "", len);
	fwrite(body, 1, len, stdout);
	putchar('\n');
}

static void
parse_one(const char *sql)
{
	MemoryContext stmt = AllocSetContextCreate(TopMemoryContext, "oracle statement", ALLOCSET_DEFAULT_SIZES);
	MemoryContext old = MemoryContextSwitchTo(stmt);

	PG_TRY();
	{
		frame('T', "", nodeToStringWithLocations(raw_parser(sql, RAW_PARSE_DEFAULT)));
	}
	PG_CATCH();
	{
		ErrorData  *e;
		char		head[32];
		char		state[6];
		int			code;

		MemoryContextSwitchTo(stmt);
		e = CopyErrorData();
		FlushErrorState();
		code = e->sqlerrcode;
		for (int i = 0; i < 5; i++)
		{
			state[i] = PGUNSIXBIT(code);
			code >>= 6;
		}
		state[5] = '\0';
		snprintf(head, sizeof(head), "%s %d", state, e->cursorpos);
		frame('E', head, e->message ? e->message : "");
	}
	PG_END_TRY();

	MemoryContextSwitchTo(old);
	MemoryContextDelete(stmt);
}

int
main(void)
{
	size_t		cap = 1 << 16;
	size_t		len = 0;
	char	   *buf = malloc(cap);
	int			c;

	MemoryContextInit();
	SetDatabaseEncoding(PG_UTF8);
	/* As the server sets it under an 8 MB stack rlimit: guc.c InitializeGUCOptions, 2048 kB. */
	set_stack_base();
	assign_max_stack_depth(2048, NULL);

	if (buf == NULL)
		return 1;
	while ((c = getchar()) != EOF)
	{
		if (len + 1 == cap)
		{
			cap *= 2;
			buf = realloc(buf, cap);
			if (buf == NULL)
				return 1;
		}
		buf[len++] = (char) c;
		if (c != '\0')
			continue;
		parse_one(buf);
		fflush(stdout);
		len = 0;
	}
	return 0;
}
