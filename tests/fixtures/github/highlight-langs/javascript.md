```javascript
import { useState } from 'react';
const re = /ab+c/gi;
export default async function fetchData(url, { retries = 3 } = {}) {
  let count = 0;
  const res = await fetch(`${url}/api?x=${count + 1}`);
  if (!res.ok) throw new Error("failed: " + res.status);
  class Foo extends Bar {
    static #priv = 42;
    constructor(a) { super(a); this.a = a ?? null; }
    get value() { return this.#priv; }
  }
  return res.json().then((data) => data.items.map(i => i.id));
}
// line comment
/* block */
console.log(typeof undefined, NaN, Infinity, 0b1010, 1_000n);
document.getElementById('x').addEventListener('click', function () {});
```
