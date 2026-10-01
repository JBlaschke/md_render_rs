```css
@import url("base.css");
:root { --main-color: #06c; }

body, html {
  margin: 0;
  font-family: "Helvetica Neue", Arial, sans-serif;
  color: var(--main-color);
}

.btn:hover > span::before {
  content: "\201C";
  transform: translateX(-50%) rotate(45deg);
  width: calc(100% - 2rem);
  background: rgba(0, 0, 0, .5) url(img.png) no-repeat !important;
}

#id[data-x="1"] { display: none; }

@media (max-width: 600px) {
  .grid { grid-template-columns: repeat(2, 1fr); }
}

@keyframes spin { from { opacity: 0; } to { opacity: 1; } }
```
