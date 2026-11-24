CFLAGS = -O2
all: gn gp ap gaps
%: %.c
	$(CC) $(CFLAGS) -o $@ $< -lm
clean:
	rm -f gn gp ap gaps
