package demo;

// The source as it exists *before* annotation processing: no getter, no
// factory, no two-argument constructor, no nested builder and no `log` field.
// The compiled mirror in `bin/` is the expanded form an annotation processor
// (Lombok's @Data/@Slf4j/@Builder) would produce from this source.
public class Person {
    private String name;

    public String mirrorField() {
        return log;
    }
}
