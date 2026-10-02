```csharp
using System;
using System.Collections.Generic;

namespace Demo
{
    [Serializable]
    public sealed class Person : IComparable<Person>
    {
        public string Name { get; init; } = "x";
        private readonly int _age;

        public Person(string name, int age) => (Name, _age) = (name, age);

        public int CompareTo(Person? other)
        {
            if (other is null) return 1;
            var s = $"Hello {Name}, {_age:D3}";
            var v = @"verbatim\path";
            return string.Compare(Name, other.Name, StringComparison.Ordinal);
        }

        public async Task<List<int>> LoadAsync()
        {
            await Task.Delay(100);
            return new List<int> { 1, 2, 3 }.Where(x => x > 1).ToList();
        }
    }
}
```
