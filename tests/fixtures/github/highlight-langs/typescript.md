```typescript
interface Props<T extends object = {}> {
  readonly id: number;
  name?: string;
  onChange: (value: T) => void;
}
type Union = 'a' | 'b' | null;
enum Color { Red = 1, Green }
export class Store<T> implements IStore {
  private items: Map<string, T> = new Map();
  constructor(private readonly api: Api) {}
  public async get(key: string): Promise<T | undefined> {
    return this.items.get(key) as T;
  }
}
function isString(x: unknown): x is string { return typeof x === "string"; }
const fn = <T,>(x: T): T[] => [x];
declare module "foo" { export const bar: number; }
@Component({ selector: 'app' })
class AppComponent {}
```
