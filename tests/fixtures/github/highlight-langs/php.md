```php
<?php
declare(strict_types=1);

namespace App\Http;

use App\Models\User;

final class UserController extends Controller
{
    private const LIMIT = 10;

    public function __construct(private UserRepository $repo) {}

    public function show(int $id): ?array
    {
        $user = $this->repo->find($id) ?? throw new NotFoundException("no user $id");
        $items = array_map(fn($x) => $x * 2, [1, 2, 3]);
        echo "Hello {$user->name}", PHP_EOL;
        return ['id' => $id, 'ok' => true, 'n' => null];
    }
}
```
