```ruby
require 'json'

module Greeter
  class Person < Base
    attr_accessor :name
    CONST = 42

    def initialize(name, *args, **opts, &blk)
      @name = name
      @@count ||= 0
      $global = nil
    end

    def greet(other = nil)
      return "Hi #{other.name}!" if other
      puts 'hello', :sym, %w[a b], /re+/i
      [1, 2, 3].each_with_index do |x, i|
        yield x * i unless x.nil?
      end
    end

    private

    def self.build = new("x")
  end
end
```
