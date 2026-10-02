```jsx
export function App({ items }) {
  const [n, setN] = useState(0);
  return (
    <div className="app" onClick={() => setN(n + 1)}>
      {items.map((i) => <Item key={i.id} {...i} />)}
      <p>Count: {n}</p>
    </div>
  );
}
```
