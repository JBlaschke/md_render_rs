```makefile
.PHONY: all clean test
CC ?= gcc
CFLAGS := -O2 -Wall
SRCS = $(wildcard src/*.c)
OBJS = $(SRCS:.c=.o)

all: app

app: $(OBJS)
	$(CC) $(CFLAGS) -o $@ $^

%.o: %.c
	$(CC) $(CFLAGS) -c $< -o $@

test: all
	./app --test
	@echo "done"

ifeq ($(OS),Windows_NT)
  EXE = .exe
endif

clean:
	rm -f $(OBJS) app  # comment
```
