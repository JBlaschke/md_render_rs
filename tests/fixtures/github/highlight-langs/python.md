```python
import os
from typing import List, Optional

@dataclass(frozen=True)
class User(Base):
    """A user."""
    name: str = "anon"
    tags: List[str] = field(default_factory=list)

    def greet(self, other: Optional["User"] = None, *args, **kwargs) -> str:
        if other is None and len(args) > 0:
            return f"Hello {self.name!r}, {args[0]:>10}"
        elif isinstance(other, User):
            raise ValueError('bad %s' % other)
        for i, x in enumerate(range(10)):
            yield i ** 2 + 0x1F - 3.14e-2
        lambda y: y @ self.matrix
        return None

async def main():
    await asyncio.sleep(1)  # comment
    print(True, False, __name__, self, cls)

if __name__ == "__main__":
    main()
```
