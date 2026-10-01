# Raw HTML

<div id="Top" class="banner" style="color:red" onclick="x()" align="center" data-x="1">
  <img src="images/logo.png" width="120" height="40" alt="Logo">
  <span title="t" aria-label="dropped">span</span>
</div>

<h2 id="Custom-Id" class="x">Raw *heading* :sparkles:</h2>

<p>Inline <kbd>Ctrl</kbd>+<kbd>C</kbd>, <abbr title="x">abbr</abbr>, <u>u</u>, <font color="red">font</font>,
<sub>sub</sub><sup>sup</sup>, <ins>ins</ins>, <mark>mark</mark>, <q cite="https://e.com">q</q>.</p>

<a name="Anchor_Name"></a>
<a href="https://example.com" target="_blank" rel="opener">external</a>
<a href="javascript:alert(1)">bad</a>

<script>alert(1)</script>
<style>p { color: red }</style>
<iframe src="https://example.com/embed"></iframe>

<!-- a comment -->

<img src="https://example.com/big.png" width="600" height="300">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://example.com/dark.png">
  <img alt="Themed" src="https://example.com/light.png">
</picture>

<pre lang="mermaid">graph LR
  X --> Y</pre>

<pre><code>raw code &amp; block</code></pre>

<pre>plain raw pre</pre>

<table>
<tr><th>Raw</th><td>table <table><tr><td>nested</td></tr></table></td></tr>
</table>

<details><p>No summary</p></details>

<input type="checkbox" checked> raw input

<video src="https://example.com/v.mp4" controls></video>
