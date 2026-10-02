```tsx
export function App({ items }: Props): JSX.Element {
  const [n, setN] = useState(0);
  return (
    <div className="app" onClick={() => setN(n + 1)}>
      {items.map((i) => <Item key={i.id} {...i} />)}
      <p>Count: {n}</p>
    </div>
  );
}
```
