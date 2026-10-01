```java
package com.example;

import java.util.*;

/**
 * Javadoc.
 */
@SuppressWarnings("unchecked")
public final class Main<T extends Comparable<T>> implements Runnable {
    private static final int MAX = 0xFF;
    protected List<String> names = new ArrayList<>();

    @Override
    public void run() {
        try {
            for (int i = 0; i < MAX; i++) {
                names.add(String.valueOf(i));
            }
        } catch (IllegalStateException | NullPointerException e) {
            throw new RuntimeException("boom", e);
        } finally {
            System.out.println(this.names.size() + 'c');
        }
    }

    public static <U> U id(U u) { return u; }

    record Pair(int a, int b) {}
    enum Kind { A, B }
}
```
