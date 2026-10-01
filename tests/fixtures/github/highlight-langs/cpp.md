```cpp
#include <iostream>
#include <vector>

namespace app {

template <typename T, std::size_t N = 4>
class Buffer : public Base {
public:
    explicit Buffer(std::initializer_list<T> init) : data_(init) {}
    virtual ~Buffer() override = default;
    T& operator[](std::size_t i) noexcept { return data_[i]; }
    constexpr static int kSize = N;
private:
    std::vector<T> data_;
};

}  // namespace app

int main() {
    auto buf = app::Buffer<int>{1, 2, 3};
    const std::string s = R"(raw)";
    std::cout << buf[0] << std::endl;
    auto lam = [&](int x) -> bool { return x > 0; };
    nullptr_t p = nullptr;
    return lam(1) ? 0 : 1;
}
```
